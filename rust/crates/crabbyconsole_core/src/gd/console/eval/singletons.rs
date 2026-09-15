use godot::{
    classes::{class_macros::private::virtuals::ZipReader::Variant, *},
    meta::ToGodot as _,
    obj::Singleton as _,
};

pub fn get_all_singletons() -> Vec<(&'static str, Variant)> {
    // Get all singletons: `Engine.get_singleton_list()`
    // Note - not all singletons are present here, because some of them are marked as experimental or just missing entirely.
    // Examples include NavigationServer2D, NavigationServer3D, JavaClassWrapper, JavaScriptBridge

    vec![
        ("AudioServer", AudioServer::singleton().to_variant()),
        ("CameraServer", CameraServer::singleton().to_variant()),
        ("ClassDB", ClassDb::singleton().to_variant()),
        ("DisplayServer", DisplayServer::singleton().to_variant()),
        ("Engine", Engine::singleton().to_variant()),
        ("EngineDebugger", EngineDebugger::singleton().to_variant()),
        (
            "GDExtensionManager",
            GDExtensionManager::singleton().to_variant(),
        ),
        ("Geometry2D", Geometry2D::singleton().to_variant()),
        ("Geometry3D", Geometry3D::singleton().to_variant()),
        ("Input", Input::singleton().to_variant()),
        ("InputMap", InputMap::singleton().to_variant()),
        ("IP", Ip::singleton().to_variant()),
        ("Marshalls", Marshalls::singleton().to_variant()),
        ("NativeMenu", NativeMenu::singleton().to_variant()),
        (
            "NavigationMeshGenerator",
            NavigationMeshGenerator::singleton().to_variant(),
        ),
        (
            "NavigationServer2DManager",
            NavigationServer2DManager::singleton().to_variant(),
        ),
        (
            "NavigationServer3DManager",
            NavigationServer3DManager::singleton().to_variant(),
        ),
        ("OS", Os::singleton().to_variant()),
        ("Performance", Performance::singleton().to_variant()),
        ("PhysicsServer2D", PhysicsServer2D::singleton().to_variant()),
        (
            "PhysicsServer2DManager",
            PhysicsServer2DManager::singleton().to_variant(),
        ),
        ("PhysicsServer3D", PhysicsServer3D::singleton().to_variant()),
        (
            "PhysicsServer3DManager",
            PhysicsServer3DManager::singleton().to_variant(),
        ),
        ("ProjectSettings", ProjectSettings::singleton().to_variant()),
        ("RenderingServer", RenderingServer::singleton().to_variant()),
        ("ResourceLoader", ResourceLoader::singleton().to_variant()),
        ("ResourceSaver", ResourceSaver::singleton().to_variant()),
        ("ResourceUID", ResourceUid::singleton().to_variant()),
        (
            "TextServerManager",
            TextServerManager::singleton().to_variant(),
        ),
        ("ThemeDB", ThemeDb::singleton().to_variant()),
        ("Time", Time::singleton().to_variant()),
        (
            "TranslationServer",
            TranslationServer::singleton().to_variant(),
        ),
        (
            "WorkerThreadPool",
            WorkerThreadPool::singleton().to_variant(),
        ),
        ("XRServer", XrServer::singleton().to_variant()),
    ]
}
