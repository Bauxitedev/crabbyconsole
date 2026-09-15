use std::{
    cmp::Reverse,
    collections::HashSet,
    env,
    ffi::OsString,
    panic::{self, AssertUnwindSafe},
    str::FromStr,
    sync::LazyLock,
};

use clap::CommandFactory as _;
use clap_complete::engine::complete;
use color_eyre::eyre::{self, eyre};
use crabbyconsole_clap::eval_definition::MainCommand;
use crabbyconsole_godot_api::{FUNCS_GDSCRIPT, FUNCS_GLOBAL_SCOPE, VariantTypePair};
use crabbyconsole_misc::{gd::async_node::AsyncGd, profile};
use godot::{classes::ClassDb, prelude::*, register::info::PropertyUsageFlags};
use indexmap::IndexSet;
use regex::Regex;
use tap::TapFallible;
use time::OffsetDateTime;

use crate::gd::console::{
    CrabConsole, lockdown::is_lockdown_enabled, util::split_console_whitespace,
};

#[derive(Debug)]
pub(super) enum SuggestionSlots {
    None,
    Search(Vec<SearchSlot>, Option<usize>), // usize = currently selected slot
    Autocomplete(Vec<AutocompleteSlot>, Option<usize>, String), // usize = currently selected slot, String = partial line
}

#[derive(Debug)]
pub(super) struct Suggestions {
    pub(super) slots: SuggestionSlots,
    pub(super) dirty: bool, // if true, the slots need to be regenerated and then redrawn in the UI
}

// TODO these are inconsistent, they should all be like {} so we get field names
#[derive(Debug, Clone)]
pub(super) enum AutocompleteType {
    Method(Vec<(String, VariantTypePair)>, VariantTypePair), // arguments -> return type
    UtilityFunc(Vec<(String, VariantTypePair)>, VariantTypePair), // arguments -> return type
    Property(VariantTypePair),
    Class { is_global: bool, is_abstract: bool }, // Type is self-describing
    BuiltInType,                                  // Type is self-describing
    Signal(Vec<(String, VariantTypePair)>, VariantTypePair), // arguments -> return type
    Clap,                                         // Clap has no types
}

#[derive(Debug, Clone)]
pub(super) struct AutocompleteSlot {
    pub(super) name: String,
    pub(super) indentation: usize,
    pub(super) help: Option<String>,
    pub(super) typ: AutocompleteType,
}

#[derive(Debug, Clone)]
pub(super) struct SearchSlot {
    pub(super) name: String,
    pub(super) timestamp: OffsetDateTime,
    pub(super) success: bool,
}

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub(super) fn handle_autocomplete_gdscript(
        self: &mut AsyncGd<Self>,
        line: &str,
    ) -> Vec<AutocompleteSlot> {
        if is_lockdown_enabled() {
            // In lockdown mode, do not show GDScript autocomplete at all
            return vec![];
        }
        let mut line = line.to_owned();

        let reduced = reduce_expression(&line);
        let (haystack, _call_depth) = self.bind().resolve_expression_autocomplete(&reduced);
        line = reduced.remaining;

        let slots = {
            let mut temp = haystack
                .iter()
                .filter_map(|slot @ AutocompleteSlot { name, .. }| {
                    let name_lower = name.to_lowercase();
                    let line_lower = line.to_lowercase();

                    if name_lower.starts_with(&line_lower) {
                        Some((true, slot.clone())) // true = perfect match
                    } else if name_lower.contains(&line_lower) {
                        Some((false, slot.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            // Deprioritize classes/built in types, since we have so many of them
            // Remember, lower number = higher priority
            temp.sort_by_cached_key(|(perfect_match, AutocompleteSlot { name, typ, .. })| {
                let rank = match typ {
                    // Deprioritize methods starting with an underscore,
                    // generally they're not useful and you don't want to call them directly.
                    AutocompleteType::Method(_, _) if name.starts_with("_") => 25,
                    AutocompleteType::Method(_, _) => 20,
                    AutocompleteType::UtilityFunc(_, _) => 30, // show after "real" methods
                    AutocompleteType::Property(_) => 10, // Highest priority, since they are less common
                    AutocompleteType::Class {
                        is_global,
                        is_abstract,
                    } => {
                        if *is_global {
                            40 // globals have higher priority
                        } else if *is_abstract {
                            100 // abstract classes have lower priority, since they're hardly useful
                        } else {
                            60
                        }
                    }
                    AutocompleteType::BuiltInType => 80,
                    AutocompleteType::Signal(_, _) => 70,
                    AutocompleteType::Clap => 0,
                };
                (
                    if *perfect_match { 0 } else { 1 }, // Show perfect matches first
                    rank,                               // Then order by rank
                    name.to_lowercase(),                // If same rank, sort alphabetically
                )
            });

            temp
        };

        slots.into_iter().map(|(_, b)| b).collect()
    }

    fn get_classes_and_globals(&self, indentation: usize) -> Vec<AutocompleteSlot> {
        let (global_names, _) = &**self.singletons;
        let global_set = global_names.as_slice().iter().collect::<IndexSet<_>>();

        ClassDb::singleton()
            .get_class_list()
            .as_slice()
            .iter()
            .map(|name| {
                let is_abstract =
                    !ClassDb::singleton().can_instantiate(&StringName::from(&name.to_string()));

                let is_global = global_set.contains(name);

                AutocompleteSlot {
                    name: name.to_string(),
                    indentation,
                    help: None,
                    typ: AutocompleteType::Class {
                        is_abstract,
                        is_global,
                    },
                }
            })
            .collect::<Vec<_>>()
    }

    fn get_built_in_types(&self, indentation: usize) -> Vec<AutocompleteSlot> {
        VariantType::values()
            .iter()
            .map(|vtype| AutocompleteSlot {
                name: vtype.godot_type_name().to_owned(),
                indentation,
                help: None,
                typ: AutocompleteType::BuiltInType,
            })
            .collect::<Vec<_>>()
    }

    fn get_utility_functions(&self, indentation: usize) -> Vec<AutocompleteSlot> {
        // You can get the entire list by doing jq -r '.utility_functions[].name' extension_api.json
        // maybe store this in a LazyCell somewhere, may be slow creating this list every time?

        profile!(FUNCS_GDSCRIPT.iter())
            .chain(profile!(FUNCS_GLOBAL_SCOPE.iter()))
            .map(|func| {
                // Only use the first line from the help string for autocomplete
                let help = func.description.split("\n").next().map(|s| s.into());

                AutocompleteSlot {
                    name: func.name.clone(),
                    indentation,
                    help,
                    typ: AutocompleteType::UtilityFunc(
                        func.arguments
                            .iter()
                            .map(|arg| {
                                (arg.name.clone(), {
                                    VariantTypePair::from_str(&arg.ty).into_ok() // infallible unwrap
                                })
                            })
                            .collect(),
                        func.return_type
                            .as_ref()
                            .map(|ty| VariantTypePair::from_str(ty).into_ok()) // infallible unwrap
                            .unwrap_or_default(),
                    ),
                }
            })
            .collect::<Vec<_>>()
    }

    fn get_variables(&self, indentation: usize) -> Vec<AutocompleteSlot> {
        self.get_vars()
            .iter()
            .map(|(name, value)| AutocompleteSlot {
                name: name.to_string(),
                indentation,
                help: None,
                typ: AutocompleteType::Property(VariantTypePair {
                    // Get the value's type
                    typ: value.get_type(),
                    class_name: StringName::from(
                        // Get the value's class name, if it is an Object
                        &value
                            .try_to::<Gd<Object>>()
                            .map(|obj| obj.get_class())
                            .unwrap_or_default(),
                    ),
                }),
            })
            .collect::<Vec<_>>()
    }

    #[tracing::instrument(skip(self))]
    pub(super) fn handle_history_search(self: &mut AsyncGd<Self>, line: &str) -> Vec<SearchSlot> {
        let history_unique = {
            let mut history = self.bind().history.clone();
            history.reverse(); // reverse it to retain only the LATEST duplicates, instead of the first
            let mut seen = HashSet::new();
            history.retain(|slot| seen.insert(slot.command.trim().to_owned())); // trailing whitespace should not be counted as unique
            history.reverse(); // reverse it again to retain only the LATEST duplicates, instead of the first
            history
        };

        // basic search alg
        let mut result: Vec<_> = history_unique
            .iter()
            .filter_map(|hist| {
                let cmd_lower = hist.command.to_lowercase();
                let line_lower = line.to_lowercase();
                if cmd_lower.starts_with(&line_lower) {
                    Some((
                        SearchSlot {
                            name: hist.command.clone(),
                            timestamp: hist.timestamp,
                            success: hist.success,
                        },
                        true, // perfect match
                    ))
                } else if cmd_lower.contains(&line_lower) {
                    Some((
                        SearchSlot {
                            name: hist.command.clone(),
                            timestamp: hist.timestamp,
                            success: hist.success,
                        },
                        false,
                    ))
                } else {
                    None
                }
            })
            .collect();

        result.sort_by_key(|(slot, perfect_match)| {
            // First sort by perfect match, then by date, then by string length
            (
                if *perfect_match { 0 } else { 1 },
                Reverse(slot.timestamp),
                slot.name.len(),
            )
        });
        result.into_iter().map(|(slot, _)| slot).collect()
    }

    #[tracing::instrument(skip_all)]
    pub(super) fn handle_autocomplete_clap(
        self: &mut AsyncGd<Self>,
        line: &str,
    ) -> Vec<AutocompleteSlot> {
        let mut args: Vec<OsString> = split_console_whitespace(line).map(OsString::from).collect();

        if line.is_empty() || line.ends_with(' ') {
            args.push("".into()); // why do we need this again?
        }

        let arg_index = args.len().saturating_sub(1); // completing last token
        let mut cmd = MainCommand::command();

        // Inject custom subcommands, so they appear in the completion list
        cmd = self.inject_subcommands(cmd);

        // We occasionally get a panic here in `clap`, so catch it.
        // Type this to trigger the panic: :watch signals --style  -u icon
        let completions = panic::catch_unwind(AssertUnwindSafe(|| {
            Ok(complete(
                &mut cmd,
                args,
                arg_index,
                env::current_dir().ok().as_deref(), // None = current dir, for file path completions
            )?)
        }))
        .map_err(panic_payload_to_eyre)
        .flatten();

        // The length of the input rounded down to the nearest space
        let rounded_input_length = second_to_last_word_end(line); // arcane black magic but okay

        completions
            .tap_err(|err| tracing::warn!(?err, "clap autocomplete failed - returning empty list"))
            .unwrap_or_default() // if completion fails, use empty vec
            .into_iter()
            .map(|completion| AutocompleteSlot {
                name: completion.get_value().to_string_lossy().to_string(),
                indentation: rounded_input_length,
                help: completion.get_help().map(|style| style.to_string()),
                typ: AutocompleteType::Clap,
            })
            .collect()
    }

    /// This parses the line, applies methods repeatedly to `self`, and returns the list of methods/props for `self`'s final type.
    /// The needle is the part after the last dot, so e.g. the needle is `i` in `f().g().h().i`
    ///
    /// Returns (method list, call depth (amount of methods called))
    fn resolve_expression_autocomplete(
        &self,
        ReduceExpressionResult {
            slots, indentation, ..
        }: &ReduceExpressionResult,
    ) -> (Vec<AutocompleteSlot>, usize) {
        let mut target_obj =
            AutocompleteTargetObj::Object(self.script_context.clone().upcast::<Object>());

        let call_chain_length = slots.len();

        // Loop through the call stack and replace the target_obj with the return value/type of the method/prop every time.
        for (call_index, call_slot) in slots.iter().enumerate() {
            let first_call = call_index == 0;

            // NOTE: ClassDB may not work in exported games:
            // "In exported release builds the debug info is not available, so the returned dictionaries will contain only method names."
            // Allegedly, in release builds, method argument names and virtual methods are missing.

            let obj = target_obj.clone(); // needed to avoid double borrow?

            let mut replace_target_obj =
                |(variant_type, class_name): (VariantType, StringName)| match class_name {
                    class_name if class_name.is_empty() => {
                        target_obj = AutocompleteTargetObj::BuiltInType(variant_type)
                    }
                    class_name if let Some(inst) = instantiate_object_by_type(&class_name) => {
                        target_obj = AutocompleteTargetObj::Object(inst);
                    }
                    class_name => {
                        target_obj = AutocompleteTargetObj::UninstantiableObject(class_name)
                    }
                };

            match call_slot {
                ReduceExpressionResultSlot::Method(method_name, _method_args) => {
                    let target = find_match(method_name, obj.get_method_list());

                    let Some(target) = target else {
                        tracing::warn!("no method on object '{obj:?}' called {method_name}");
                        return (vec![], 0);
                    };

                    replace_target_obj(extract_type_method(target));
                }
                ReduceExpressionResultSlot::Prop(prop_name) => {
                    let mut prop_list = filter_garbage_props(obj.get_property_list());

                    if first_call {
                        // If this is the first call, we inject globals so we get proper autocompletion for stuff like `Engine.`
                        // TODO maybe inject built in types here too?
                        // Right now if you type `Vector2.` it says "there is no property on object CrabbyConsole called Vector2"
                        // We do want that to work though!
                        prop_list.extend_array(&self.singletons_to_prop_list());

                        // Also inject the variable list, so we can do `crab.` to get autocomplete for the crab
                        prop_list.extend_array(&self.variables_to_prop_list());
                    }

                    let target = find_match(prop_name, prop_list);

                    let Some(target) = target else {
                        tracing::debug!("no property on object '{obj:?}' called {prop_name}");
                        return (vec![], 0);
                    };

                    replace_target_obj(extract_type_prop(target));
                }
            }
        }

        let mut result = target_obj.inspect(*indentation);

        if call_chain_length == 0 {
            // We only show classes, globals, built in types, and utility functions in the autocomplete list if there is no call chain yet.
            // So they show up when you type `foo` but not `foo.` or `foo.bar.`
            result.append(&mut self.get_classes_and_globals(*indentation));
            result.append(&mut self.get_built_in_types(*indentation));
            result.append(&mut self.get_utility_functions(*indentation));
            result.append(&mut self.get_variables(*indentation));
        }
        (result, call_chain_length)
    }

    /// Convert the globals to the same "format" (`Dictionary`) returned by `get_property_list()` and `get_method_list()`.
    fn singletons_to_prop_list(&self) -> Array<VarDictionary> {
        let (global_names, global_values) = &**self.singletons;
        tracing::debug!("injecting {} globals", global_names.len());
        global_names
            .as_slice()
            .iter()
            .zip(global_values.iter_shared())
            .map(|(n, v)| to_prop(n.to_string(), v))
            .collect::<Array<_>>()
    }

    /// Convert the variables to the same "format" (`Dictionary`) returned by `get_property_list()` and `get_method_list()`.
    fn variables_to_prop_list(&self) -> Array<VarDictionary> {
        let vars = self.get_vars();
        tracing::debug!("injecting {} variables", vars.len());
        vars.into_iter()
            .map(|(n, v)| to_prop(n.to_string(), v))
            .collect::<Array<_>>()
    }
}

/// Convert a prop to the same "format" (aka a Dictionary) returned by `get_property_list()` and `get_method_list()`.
///
/// WARNING - make sure the dictionary stores name as a `String`, NOT a `StringName`, or it will panic later in `find_match()`.
/// So `n` should always be passed as a `String`, not `StringName`.
fn to_prop(n: String, v: Variant) -> VarDictionary {
    VarDictionary::from_iter([
        ("name".to_variant(), n.to_variant()), // <-- Important - convert n from StringName to String, or find_match will panic later
        ("type".to_variant(), v.get_type().to_variant()),
        (
            "class_name".to_variant(),
            v.try_to::<Gd<Object>>()
                .map(|obj| StringName::from(&obj.get_class()).to_variant())
                .unwrap_or_default(),
        ),
    ])
}

#[derive(Debug, Clone)]
enum AutocompleteTargetObj {
    Object(Gd<Object>),               // Every object except...
    UninstantiableObject(StringName), // Uninstantiable types go here (e.g. DisplayServer, ProjectSettings), we use ClassDB.class_get_method_list for those
    // Having the original Variant is useful, because then we can access console's gdscript props/functions
    // Wait, aren't those stored in the ClassDB as well?
    BuiltInType(VariantType),
}

impl AutocompleteTargetObj {
    /*
    There are two ways to get the methods/props of an object:
    1. obj.get_method_list()/get_property_list()
    2. ClassDB.class_get_method_list(obj.get_class())/class_get_property_list(obj.get_class())
    However, they return slightly different results. 2 is a subset of 1, so 1 always return more methods/props than 2.

    Allegedly, the following methods are returned by 1, but not by 2:
    - Any custom function defined in a GDScript script attached to obj.

    Allegedly, the following props are returned by 1, but not by 2:
    - Script exported variables in a GDScript script attached to obj.
    - The `script` property.
    - metadata/... properties.
    - PROPERTY_USAGE_CATEGORY headers.
    - Dynamic properties from a C++/GDExtension _get_property_list() override (e.g. for ShaderMaterials, the shader_parameter/... properties).
    */

    // TODO it would be better to deserialize the dict returned by get_property_list and class_get_property_list into an actual struct.
    // Go make a serde DictionaryDeserializer so it works on any Dictionary.

    fn get_property_list(&self) -> Array<VarDictionary> {
        match self {
            AutocompleteTargetObj::Object(obj) => obj.get_property_list(),
            AutocompleteTargetObj::UninstantiableObject(class_name) => {
                ClassDb::singleton().class_get_property_list(class_name)
            }
            AutocompleteTargetObj::BuiltInType(typ) => get_prop_list_for_built_in_type(typ),
        }
    }

    fn get_method_list(&self) -> Array<VarDictionary> {
        match self {
            AutocompleteTargetObj::Object(obj) => obj.get_method_list(),
            AutocompleteTargetObj::UninstantiableObject(class_name) => {
                ClassDb::singleton().class_get_method_list(class_name)
            }
            AutocompleteTargetObj::BuiltInType(typ) => get_method_list_for_built_in_type(typ), // TODO there is no way to get the list of methods/props for Vector, String, etc
        }
    }

    fn get_signal_list(&self) -> Array<VarDictionary> {
        match self {
            AutocompleteTargetObj::Object(obj) => obj.get_signal_list(), // similar to obj.get_method_list()
            AutocompleteTargetObj::UninstantiableObject(class_name) => {
                ClassDb::singleton().class_get_signal_list(class_name)
            }
            AutocompleteTargetObj::BuiltInType(_) => array![], // Built in types don't have signals
        }
    }

    /// Inspects the object and lists all of its methods, props, and signals.
    fn inspect(self, indentation: usize) -> Vec<AutocompleteSlot> {
        let method_list = self.get_method_list();
        let prop_list = filter_garbage_props(self.get_property_list());
        let signal_list = self.get_signal_list();

        // no need to filter here, that happens later
        method_list
            .iter_shared()
            .map(|method| {
                let name = method.get("name").unwrap().to::<String>();
                let args = method.get("args").unwrap().to::<Array<VarDictionary>>();

                let args_full = args
                    .iter_shared()
                    .map(|arg| {
                        let name = arg.get("name").unwrap().to::<String>();
                        let typ = arg.get("type").unwrap().to::<VariantType>();
                        let class_name = arg.get("class_name").unwrap().to::<StringName>();

                        (name, VariantTypePair { typ, class_name })
                    })
                    .collect();

                let returnal = method.get("return").unwrap().to::<VarDictionary>();
                let return_type = returnal.get("type").unwrap().to::<VariantType>();
                let return_class_name = returnal.get("class_name").unwrap().to::<StringName>();

                AutocompleteSlot {
                    name,
                    indentation,
                    help: None, // empty for now, this is not possible -> https://github.com/godotengine/godot/pull/112628
                    typ: AutocompleteType::Method(
                        args_full,
                        VariantTypePair {
                            typ: return_type,
                            class_name: return_class_name,
                        },
                    ),
                }
            })
            .chain(prop_list.iter_shared().map(|prop| {
                let name = prop.get("name").unwrap().to::<String>();
                let return_type = prop.get("type").unwrap().to::<VariantType>();
                let return_class_name = prop.get("class_name").unwrap().to::<StringName>();

                AutocompleteSlot {
                    name,
                    indentation,
                    help: None,
                    typ: AutocompleteType::Property(VariantTypePair {
                        typ: return_type,
                        class_name: return_class_name,
                    }),
                }
            }))
            .chain(signal_list.iter_shared().map(|signal| {
                // TODO copy pasted from above...
                // it seems signals never have a return type...
                let name = signal.get("name").unwrap().to::<String>();
                let args = signal.get("args").unwrap().to::<Array<VarDictionary>>();

                let args_full = args
                    .iter_shared()
                    .map(|arg| {
                        let name = arg.get("name").unwrap().to::<String>();
                        let typ = arg.get("type").unwrap().to::<VariantType>();
                        let class_name = arg.get("class_name").unwrap().to::<StringName>();

                        (name, VariantTypePair { typ, class_name })
                    })
                    .collect();

                let returnal = signal.get("return").unwrap().to::<VarDictionary>();
                let return_type = returnal.get("type").unwrap().to::<VariantType>();
                let return_class_name = returnal.get("class_name").unwrap().to::<StringName>();

                AutocompleteSlot {
                    name,
                    indentation,
                    help: None,
                    typ: AutocompleteType::Signal(
                        args_full,
                        VariantTypePair {
                            typ: return_type,
                            class_name: return_class_name,
                        },
                    ),
                }
            }))
            .collect::<Vec<_>>()
    }
}

// TODO there is no way to get the list of methods/props for Vector, String, etc
fn get_prop_list_for_built_in_type(typ: &VariantType) -> Array<VarDictionary> {
    /*
    get_property_list:
       Returns the object's property list as an Array of dictionaries. Each Dictionary contains the following entries:
           name is the property's name, as a String;
           class_name is an empty StringName, unless the property is VariantType::OBJECT and it inherits from a class;
           type is the property's type, as an int (see [enum Variant.Type]);
           hint is how the property is meant to be edited (see [enum PropertyHint]);
           hint_string depends on the hint (see [enum PropertyHint]);
           usage is a combination of [enum PropertyUsageFlags].
    */
    match typ {
        &VariantType::VECTOR3 => vec![
            vdict! {
                "name" =>  "x",
                "class_name"=> &StringName::from(""),
                "type" => VariantType::FLOAT,
                "usage" => PropertyUsageFlags::NONE, // wrong but okay
            },
            vdict! {
                "name" =>  "y",
                "class_name"=>  &StringName::from(""),
                "type" => VariantType::FLOAT,
                "usage" => PropertyUsageFlags::NONE, // wrong but okay
            },
            vdict! {
                "name" =>  "z",
                "class_name"=>  &StringName::from(""),
                "type" => VariantType::FLOAT,
                "usage" => PropertyUsageFlags::NONE, // wrong but okay
            },
        ],
        _ => vec![],
    }
    .to_godot()
}

fn get_method_list_for_built_in_type(typ: &VariantType) -> Array<VarDictionary> {
    /*
    get_method_list:
       Returns this object's methods and their signatures as an Array of dictionaries. Each Dictionary contains the following entries:

           name is the name of the method, as a String;
           args is an Array of dictionaries representing the arguments;
           default_args is the default arguments as an Array of variants;
           flags is a combination of [enum MethodFlags];
           id is the method's internal identifier int;
           return is the returned value, as a Dictionary;

       Note: The dictionaries of args and return are formatted identically to the results of get_property_list, although not all entries are used.
    */
    match typ {
        &VariantType::VECTOR3 => vec![vdict! {
            "name" => "normalized",
            "args" => &Array::<VarDictionary>::new(), // can't use array! due to type inference
            "default_args" => &Array::<VarDictionary>::new(),
            "class_name" => "",
            "return" => &vdict! {
                "class_name" =>  &StringName::from(""),
                "type" => VariantType::FLOAT,
            }
        }],
        _ => vec![],
    }
    .to_godot()
}

fn find_match(name: &str, input: Array<VarDictionary>) -> Option<VarDictionary> {
    input
        .iter_shared()
        .find(|method| method.get("name").unwrap().to::<String>() == name)
}

fn extract_type_method(target: VarDictionary) -> (VariantType, StringName) {
    let return_info = target.get("return").unwrap().to::<VarDictionary>();
    let variant_type = resolve_return_type(&return_info);

    let class_name = return_info.get("class_name").unwrap().to::<StringName>();
    tracing::debug!(?class_name);

    (variant_type, class_name)
}

fn extract_type_prop(target: VarDictionary) -> (VariantType, StringName) {
    let variant_type = resolve_return_type(&target);

    // Important - use unwrap_or_default() and try_to here, since "class_name" can be Variant::nil if type == PackedByteArray
    let class_name = target
        .get("class_name")
        .unwrap()
        .try_to::<StringName>()
        .unwrap_or_default();
    tracing::debug!(?class_name);

    (variant_type, class_name)
}

/// Resolves a method's return info dictionary down to its `VariantType`.
fn resolve_return_type(return_info: &VarDictionary) -> VariantType {
    let type_int = return_info.get("type").unwrap().to::<i32>();
    VariantType::from_ord(type_int)
}

fn instantiate_object_by_type(class_name: &StringName) -> Option<Gd<Object>> {
    if class_name.is_empty() {
        tracing::warn!("OBJECT return type but no class_name provided");
        return None;
    }

    let db = ClassDb::singleton();

    if !db.class_exists(class_name) {
        tracing::warn!("Unknown class, cannot instantiate: {class_name}");
        return None;
    }

    if !db.can_instantiate(class_name) {
        tracing::warn!("Uninstantiable class, cannot instantiate: {class_name}");
        return None;
    }

    // These return can_instantiate(...) == true, but still break when you try to instantiate them, so reject them anyway
    let custom_exceptions = ["ProjectSettings", "AudioServer"];

    if custom_exceptions.iter().any(|name| *class_name == *name) {
        tracing::warn!("Uninstantiable class (custom exception), cannot instantiate: {class_name}");
        return None;
    }

    Some(db.instantiate(class_name).to::<Gd<Object>>())
}

/// arcane black magic
fn second_to_last_word_end(s: &str) -> usize {
    let mut words: Vec<usize> = vec![]; // (end) byte indices

    // collect non-whitespace words
    let mut in_word = false;
    for (i, c) in s.char_indices() {
        if !c.is_whitespace() {
            if !in_word {
                in_word = true;
            }
        } else {
            if in_word {
                words.push(i);
                in_word = false;
            }
        }
    }
    if in_word {
        words.push(s.len());
    }

    // trailing whitespace counts as a word
    if s.ends_with(|c: char| c.is_whitespace()) {
        words.push(s.len()); // dummy trailing "word"
    }

    if words.len() < 2 {
        return 1;
    }

    words[words.len() - 2] + 2
}

/// Repeatedly matches `name(args).` or `name.` at the front of the string,
/// stripping each match off and collecting call/prop slots, until no more
/// matches are found. Returns the collected slots plus whatever's left over.
///
/// Note - does not work for expressions with nested braces like `f(g())`.
/// For that we need to be able to parse balanced braces.
/// That means we need either recursive regexes like PCRE2, or a proper parser like `nom`.
/// Setting the regex to greedy mode won't work either, because then it will parse `f().g().b().`
/// as a function `f` with argument `).g().b(`
fn reduce_expression(input: &str) -> ReduceExpressionResult {
    /// This regex matches `foo().` or `bar(1, 2, 3).` or `baz.`.
    /// Using a `LazyLock` to cache the compilation of the regex.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_]*)(?:\((?P<args>[^()]*)\))?\.").unwrap()
    });

    let mut slots = Vec::new();
    let mut remaining = input.to_string();

    let mut indentation = 0;
    while let Some(caps) = RE.captures(&remaining) {
        let name = caps["name"].to_string();

        // args is only present if the "(...)" part matched at all.
        // absence means this was a bare prop access, not a call.
        let slot = match caps.name("args") {
            Some(args) => ReduceExpressionResultSlot::Method(name, args.as_str().to_string()),
            None => ReduceExpressionResultSlot::Prop(name),
        };

        slots.push(slot);

        // how many characters were cut off
        let matched_len = caps.get(0).unwrap().as_str().chars().count();

        // replace() only replaces the first match by default, which is the one we just captured
        // it should be byte-safe, no manual slicing
        remaining = RE.replace(&remaining, "").into_owned();

        indentation += matched_len;
    }

    ReduceExpressionResult {
        slots,
        indentation,
        remaining,
    }
}

struct ReduceExpressionResult {
    slots: Vec<ReduceExpressionResultSlot>,
    indentation: usize,
    remaining: String,
}

enum ReduceExpressionResultSlot {
    Method(String, String), // Method name, method args
    Prop(String),           // Prop name
}

/// Filter out these garbage prop types:
///
/// - `PROPERTY_USAGE_GROUP` = 64
///   Used to group properties together in the editor. See `EditorInspector`.
/// - `PROPERTY_USAGE_CATEGORY` = 128
///   Used to categorize properties together in the editor.
/// - `PROPERTY_USAGE_SUBGROUP` = 256
///   Used to group properties together in the editor in a subgroup (under a group). See `EditorInspector`.
///
/// Note: this method may be faster if it returned a `Vec<VarDictionary>` instead of re-constructing an `Array` again,
/// because that may need calling into Godot again.
fn filter_garbage_props(prop_list: Array<VarDictionary>) -> Array<VarDictionary> {
    let len_before = prop_list.len();
    let prop_list: Array<_> = prop_list
        .iter_shared()
        .filter(|p| {
            let name = p.get("name").unwrap().to::<String>();

            // Skip special props like "metadata/foo" and "shader_parameter/foo"
            // You can't use those directly without causing a GDScript syntax error.
            // You have to write get("metadata/foo") instead.
            if name.contains("/") {
                return false;
            }

            let usage = p.get("usage").unwrap().to::<PropertyUsageFlags>();

            let mask = PropertyUsageFlags::GROUP
                | PropertyUsageFlags::CATEGORY
                | PropertyUsageFlags::SUBGROUP;

            !usage.is_set(mask)
        })
        .collect();
    tracing::debug!("removed {} garbage props", len_before - prop_list.len());

    prop_list
}

fn panic_payload_to_eyre(payload: Box<dyn std::any::Any + Send>) -> eyre::Report {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic (non-string payload)".to_string());
    eyre!("panicked: {msg}")
}
