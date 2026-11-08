#![forbid(unsafe_code)]

//! cksk's own library half: status feeds plus the boards that render them -- all kept
//! entirely display-server-free (no Wayland, no PAM), so a future web build can reuse them
//! just as easily as `src/bin/cksk.rs` does. That binary is the only thing in this crate
//! that turns a rendered board frame into the kiosk-socket wire protocol.
//!
//! Boards are named for what they show, not for where they are displayed: the show rotates
//! several of them (`show`), and each one owns its render behind the same `(state, size)`
//! shape over shared `chrome`.

pub mod chrome;
pub mod gatus;
pub mod groups;
pub mod layout;
pub mod model;
pub mod overview;
pub mod show;
pub mod system;

pub use chrome::{Fonts, Header};
pub use gatus::GatusSource;
pub use model::Endpoint;
pub use show::{detail_pages, Page, Show, DWELL_SECS};
