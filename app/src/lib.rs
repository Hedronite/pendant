//! Pendant's phone app: an nsh shell over the bridge.
//!
//! - [`App`] is the root `nsh.json` names. It holds the navigation stack.
//! - [`net`] talks to the bridge (nsh `fetch`, and the WebSocket envelope).
//! - [`agents`] holds the screens, and [`theme`] the colours.
//! - On the host only, the notes store (and its Lapis link) stays the
//!   library it was, so `pendant_app::{Config, Store}` and its tests are
//!   unchanged. The phone builds have no Lapis link until D1.

pub mod agents;
pub mod net;
pub mod theme;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[path = "notes/store.rs"]
mod store;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use store::*;

// On the phones the store is not built, so the device key mounts here.
#[cfg(any(target_os = "ios", target_os = "android"))]
#[path = "platform/keystore.rs"]
pub mod keystore;

use nsh_base::nav_stack;
use nsh_core::prelude::*;

/// The app's UI set. `nsh package` registers it in the runner.
#[native_module("pendant/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// A page on the navigation stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The agent list: the bottom page.
    Agents,
    /// One agent, by the id the bridge gives it.
    Agent(String),
    /// Blocked and done events.
    Inbox,
}

impl Route {
    /// The page's key on the stack. Unique for each route.
    pub fn key(&self) -> String {
        match self {
            Self::Agents => "agents".to_owned(),
            Self::Agent(id) => format!("agent:{id}"),
            Self::Inbox => "inbox".to_owned(),
        }
    }
}

/// What a screen uses to move: push a page, or go back one.
#[derive(Clone, Copy)]
pub struct Nav {
    routes: Signal<Vec<Route>>,
}

impl Nav {
    pub fn push(&self, route: Route) {
        self.routes.update(|routes| {
            // A page already on the stack is not stacked twice.
            if let Some(at) = routes.iter().position(|open| open == &route) {
                routes.truncate(at + 1);
            } else {
                routes.push(route);
            }
        });
    }

    /// Back one page; the agent list always stays.
    pub fn back(&self) {
        self.routes.update(|routes| {
            if routes.len() > 1 {
                routes.pop();
            }
        });
    }

    pub fn depth(&self) -> usize {
        self.routes.with(Vec::len)
    }
}

/// The bridge this build talks to (`PENDANT_BRIDGE_URL` at build time).
pub fn bridge() -> Result<net::Bridge, net::ConfigError> {
    net::Bridge::from_build()
}

/// The root component.
#[component]
pub fn App() -> impl IntoElement {
    // The live link: snapshot and inbox polling, and the WebSocket worker.
    // Screens read it with `net::live::live()`.
    net::live::start();
    let routes = signal(vec![Route::Agents]);
    let nav = Nav { routes };
    nav_stack().flex_1().child(each(
        move || routes.get(),
        |route: &Route| route.key(),
        move |route, index| {
            let page = match route.get_untracked() {
                Route::Agents => agents::list(nav),
                Route::Agent(id) => agents::detail(nav, id),
                Route::Inbox => agents::inbox(nav),
            };
            // The user went back natively (swipe, Android back): drop the
            // page and any above it.
            page.on_dismiss(move || routes.update(|r| r.truncate(index.get_untracked().max(1))))
        },
    ))
}
