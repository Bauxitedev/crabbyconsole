use std::{
    fmt,
    ops::{Deref, DerefMut},
};

use godot::prelude::*;
use indexmap::IndexMap;

/// Makes the Debug impl of `T` "opaque", so it only prints the type name when you call `Debug` on it, not its full innards.
/// Useful for stuff like `ConvexPolyhedron` and `TriMesh` to avoid printing the entire vertex list.
/// Also useful for structs that don't have a meaningful `Debug` impl, such as function pointers.
///
/// Cool idea: why not make it store a closure, so we can have different behavior per type?
/// Or just use `min_specialization`, see below
#[derive(Hash, Clone, Default)]
pub struct OpaqueDebug<T>(pub T);

// Update - now using min_specialization so we can do this:
impl<T> fmt::Debug for OpaqueDebug<Vec<T>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Vec<{}> with {} items",
            std::any::type_name::<T>(),
            self.len()
        )
    }
}

impl<K, V> fmt::Debug for OpaqueDebug<IndexMap<K, V>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "IndexMap<{}, {}> with {} items",
            std::any::type_name::<K>(),
            std::any::type_name::<V>(),
            self.len()
        )
    }
}

impl<K, V> fmt::Debug for OpaqueDebug<mini_moka::unsync::Cache<K, V>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mini_moka::unsync::Cache<{}, {}> with {} items (policy: {:?})",
            std::any::type_name::<K>(),
            std::any::type_name::<V>(),
            self.entry_count(),
            self.policy()
        )
    }
}

impl std::fmt::Debug for OpaqueDebug<Callable> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only describe callables whose target is actually alive.
        // is_valid() checks object existence + method name validity.
        // If you don't do this it will crash the game!
        if self.is_valid() {
            write!(f, "{:?}.{:?}", self.object(), self.method_name())?;
        } else {
            write!(f, "<invalid callable>")?;
        }

        Ok(())
    }
}

impl<T> fmt::Debug for OpaqueDebug<T> {
    // Here `default` means "impl for any T we haven't manually specified above"
    // This feature is called "min_specialization"
    default fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (opaque)", std::any::type_name::<T>())
    }
}

impl<T> Deref for OpaqueDebug<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for OpaqueDebug<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

// --------------------

/// Implement this trait for your Rust class so we can inspect it via the console.
/// Do not forget to add the `#[godot_dyn]` annotation when you do this.
pub trait ConsoleDebug: std::fmt::Debug {
    fn console_debug(&self, pretty: bool) -> String {
        if pretty {
            format!("{self:#?}")
        } else {
            format!("{self:?}")
        }
    }
}

// --------------------

/// This trait basically does `format!("{result:?}")` except with some extra safety checks.
/// This is needed to prevent crashes when visualizing some types,
/// notably `Callable` (segfaults) and `GDScriptNativeObject` (panics).
pub trait SafeDebug {
    fn safe_debug(&self) -> String;
}

impl SafeDebug for Callable {
    fn safe_debug(&self) -> String {
        if self.is_valid() {
            format!("{self:?}")
        } else {
            tracing::warn!("prevented segfault in Callable::debug");
            "Callable(<invalid>)".to_string()
        }
    }
}

impl SafeDebug for Variant {
    fn safe_debug(&self) -> String {
        if let Ok(obj) = self.try_to_relaxed::<Gd<Object>>() {
            // Note - GDScriptNativeClass is not exposed yet in godot-rust,
            // so do string comparison here instead of try_to_relaxed.

            if obj.get_class() == "GDScriptNativeClass" {
                tracing::warn!("prevented panic in GDScriptNativeClass::debug");
                return "GDScriptNativeClass(<unsafe debug>)".to_string();
            }
        }

        if let Ok(callable) = self.try_to::<Callable>() {
            return callable.safe_debug();
        }

        format!("{self:?}")
    }
}

impl<T: SafeDebug, E: fmt::Debug> SafeDebug for Result<T, E> {
    fn safe_debug(&self) -> String {
        match self {
            Ok(v) => format!("Ok({})", v.safe_debug()),
            Err(e) => format!("Err({e:?})"),
        }
    }
}
