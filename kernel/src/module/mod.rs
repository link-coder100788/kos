#[repr(C)]
pub struct ModuleDescriptor {
    pub name: &'static str,
    pub version: u32,
    pub init: fn(),
    pub deinit: fn(),
}

