#[cfg(windows)]
mod support;

#[cfg(windows)]
use support::acceptance::AcceptanceHarness;

/// Direct2D, DirectWrite, and WIC must load only when a preview opens, never through the import
/// table: a static import would load them into every launch and slow cold startup.
#[cfg(windows)]
#[test]
fn binary_does_not_statically_import_preview_graphics_libraries() {
    AcceptanceHarness::new().assert_no_preview_imports();
}
