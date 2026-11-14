//! The agent screens. Frontend owns this file; these are placeholders that
//! keep the shell's navigation contract:
//!
//! - `list(nav)` is the bottom page; `nav.push(Route::Agent(id))` opens one
//!   agent and `nav.push(Route::Inbox)` the inbox.
//! - `detail(nav, id)` and `inbox(nav)` are pushed pages.
//!
//! Data comes from [`crate::bridge`] and [`crate::net`].

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::{Nav, Route};

pub fn list(nav: Nav) -> Screen {
    nav_page().title("Agents").child(
        view()
            .flex_1()
            .p_4()
            .gap_3()
            .child(text("Agents"))
            .child(text("Inbox").on_press(move || nav.push(Route::Inbox))),
    )
}

pub fn detail(_nav: Nav, id: String) -> Screen {
    nav_page()
        .title(id.clone())
        .child(view().flex_1().p_4().child(text(id)))
}

pub fn inbox(_nav: Nav) -> Screen {
    nav_page()
        .title("Inbox")
        .child(view().flex_1().p_4().child(text("Inbox")))
}
