//! The interactive terminal UI.

mod app;
mod boxed;
mod card;
mod cards;
mod chart;
mod choice;
mod command;
mod dashboard;
mod editor;
mod effort;
mod feed;
mod forums;
mod frame;
mod header;
mod keymap;
mod layout;
mod login;
mod logo;
mod matrix;
mod palette;
mod paste;
mod picker;
mod questions;
mod queue;
mod screen;
pub mod startup;
mod status;
mod toast;
mod ui;
mod view;
mod welcome;

pub use app::run;
