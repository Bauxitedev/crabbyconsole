use std::{str::FromStr, sync::LazyLock};

use color_eyre::eyre::eyre;
use godot::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct VariantTypePair {
    pub typ: VariantType,
    pub class_name: StringName, // StringName is the class name if VariantType == OBJECT, else "".
}

impl Default for VariantTypePair {
    fn default() -> Self {
        Self {
            typ: VariantType::NIL,
            class_name: StringName::default(),
        }
    }
}

impl FromStr for VariantTypePair {
    type Err = !; // infallible

    // It looks like in FUNCS_GLOBAL_SCOPE, the set of all possible types is:
    // "Object",
    // "PackedByteArray",
    // "PackedInt64Array",
    // "RID",
    // "String",
    // "Variant",
    // "bool",
    // "float",
    // "int"
    // So make sure VariantTypePair::from_str works for them!

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(typ) = parse_variant_type(s) {
            Ok(Self {
                typ,
                class_name: StringName::default(), // ""
            })
        } else {
            // If variant type fails to parse, assume it's an Object
            Ok(Self {
                typ: VariantType::OBJECT,
                class_name: s.into(),
            })
        }
    }
}

impl VariantTypePair {
    pub fn nice_type_name(&self) -> String {
        if self.typ == VariantType::OBJECT {
            // TODO self.class_name is empty on metadata props like `metadata/a`? Weird...
            self.class_name.to_string()
        } else {
            self.typ.godot_type_name().to_owned()
        }
    }
}

static VARIANT_TYPES: LazyLock<Vec<VariantType>> = LazyLock::new(|| VariantType::values().to_vec());

pub fn parse_variant_type(name: &str) -> color_eyre::Result<VariantType> {
    // TODO handle types with a , in between them, happens for Sprite2D.material
    VARIANT_TYPES
        .iter()
        .find(|ty| ty.godot_type_name() == name)
        .copied()
        .ok_or_else(|| eyre!("unknown variant type: {name}"))
}

#[derive(Deserialize)]
pub struct UtilityFunction {
    pub name: String,

    // Note - `return_type` can be missing, so just default to None in that case
    #[serde(default)]
    pub return_type: Option<String>,

    pub is_vararg: bool,

    // Note - `arguments` can be missing, so just default to empty list in that case
    #[serde(default)]
    pub arguments: Vec<Argument>,

    pub description: String,
}

#[derive(Deserialize)]
pub struct Argument {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// The first time you call this it takes about 0.13ms to parse the json. Afterwards it's <0.001ms.
/// Seems fast enough.
pub static FUNCS_GLOBAL_SCOPE: LazyLock<Vec<UtilityFunction>> = LazyLock::new(|| {
    // Copy-paste the output into `funcs_global` from running this:
    //      godot --headless --dump-extension-api-with-docs
    //      jq -r '.utility_functions' extension_api.json

    // TODO actually we may be able to use
    // gdextension_api::version_4_5::load_extension_api_json();

    serde_json::from_slice(include_bytes!("../data/funcs_global.json"))
        .expect("failed to parse funcs_global.json")
});

/// This one was generated mostly manually.
/// See <https://docs.godotengine.org/en/stable/classes/class_%40gdscript.html>.
pub static FUNCS_GDSCRIPT: LazyLock<Vec<UtilityFunction>> = LazyLock::new(|| {
    serde_json::from_slice(include_bytes!("../data/funcs_gdscript.json"))
        .expect("failed to parse funcs_gdscript.json")
});
