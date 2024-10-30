use owo_colors::{OwoColorize, Stream};
use std::fmt::Display;

pub trait InfoColors: Display {
    fn as_important_value(&self) -> impl Display;
}

impl<D: Display> InfoColors for D {
    fn as_important_value(&self) -> impl Display {
        self.if_supports_color(Stream::Stderr, |s| s.bright_blue())
    }
}
