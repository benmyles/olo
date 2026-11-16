//! The Interface Kit's macros (olo-interface alone uses them):
//!
//! - the `Styled` helpers (`.flex()`, `.p_2()`, `.border_1()`…),
//!   generated into `styled.rs` (`styles`);
//! - `#[derive(Refineable)]`: a style's refinement, a type of the same
//!   fields each optional, laid over a base (`refineable`; the trait is
//!   `olo_interface::refineable::Refineable`).
//!
//! What they generate names the kit's types by `crate::`, as only
//! olo-interface uses them. (They began as Zed's gpui-macros and
//! gpui_derive_refineable, Apache-2.0: LICENSE-APACHE. Olo's own now.)

mod refineable;
mod styles;

use proc_macro::TokenStream;

/// The style helpers (`styled.rs`).
#[proc_macro]
#[doc(hidden)]
pub fn style_helpers(input: TokenStream) -> TokenStream {
    styles::style_helpers(input)
}

/// Generates methods for visibility styles.
#[proc_macro]
pub fn visibility_style_methods(input: TokenStream) -> TokenStream {
    styles::visibility_style_methods(input)
}

/// Generates methods for margin styles.
#[proc_macro]
pub fn margin_style_methods(input: TokenStream) -> TokenStream {
    styles::margin_style_methods(input)
}

/// Generates methods for padding styles.
#[proc_macro]
pub fn padding_style_methods(input: TokenStream) -> TokenStream {
    styles::padding_style_methods(input)
}

/// Generates methods for position styles.
#[proc_macro]
pub fn position_style_methods(input: TokenStream) -> TokenStream {
    styles::position_style_methods(input)
}

/// Generates methods for overflow styles.
#[proc_macro]
pub fn overflow_style_methods(input: TokenStream) -> TokenStream {
    styles::overflow_style_methods(input)
}

/// Generates methods for cursor styles.
#[proc_macro]
pub fn cursor_style_methods(input: TokenStream) -> TokenStream {
    styles::cursor_style_methods(input)
}

/// Generates methods for border styles.
#[proc_macro]
pub fn border_style_methods(input: TokenStream) -> TokenStream {
    styles::border_style_methods(input)
}

/// Generates methods for box shadow styles.
#[proc_macro]
pub fn box_shadow_style_methods(input: TokenStream) -> TokenStream {
    styles::box_shadow_style_methods(input)
}

/// `#[derive(Refineable)]`, with its `#[refineable(..)]` attributes:
/// see `refineable`.
#[proc_macro_derive(Refineable, attributes(refineable))]
pub fn derive_refineable(input: TokenStream) -> TokenStream {
    refineable::derive_refineable(input)
}
