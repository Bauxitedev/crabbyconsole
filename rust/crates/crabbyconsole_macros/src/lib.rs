use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Put this on your test like `#[godot_test]` to make a integration test that runs inside of Godot
///
/// TODO - allow making skippable/async tests? like `#[skip]` and `#[godot_test(async)]` or something?
#[proc_macro_attribute]
pub fn godot_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let fn_name_str = fn_name.to_string();

    quote! {
        #input_fn

        ::inventory::submit! {
            crate::test_registry::GodotTest {
                name: #fn_name_str,
                func: #fn_name,
            }
        }
    }
    .into()
}
