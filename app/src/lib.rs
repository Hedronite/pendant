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

/// The variable naming a page to open over the list at launch.
pub const START_VAR: &str = "PENDANT_START";

/// The stack at launch: the agent list, plus the page `start` names
/// (`inbox` or `agent:<id>`), read from [`START_VAR`] at run time. A
/// simulator launch passes it as `SIMCTL_CHILD_PENDANT_START`, so a check
/// can open a page without taps. Anything else opens the list alone.
pub fn start_routes(start: Option<&str>) -> Vec<Route> {
    let mut routes = vec![Route::Agents];
    match start.map(str::trim) {
        Some("inbox") => routes.push(Route::Inbox),
        Some(other) => {
            if let Some(id) = other.strip_prefix("agent:").filter(|id| !id.is_empty()) {
                routes.push(Route::Agent(id.to_owned()));
            }
        }
        None => {}
    }
    routes
}

/// The root component.
#[component]
pub fn App() -> impl IntoElement {
    // The live link: snapshot and inbox polling, and the WebSocket worker.
    // Screens read it with `net::live::live()`.
    net::live::start();
    let start = std::env::var(START_VAR).ok();
    let routes = signal(start_routes(start.as_deref()));
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
            page.on_dismiss(move || {
                let at = index.get_untracked().max(1);
                net::live::trace(&format!("dismissed page {at}"));
                routes.update(|r| r.truncate(at));
            })
        },
    ))
}

#[cfg(test)]
mod start_tests {
    use super::{start_routes, Route};

    #[test]
    fn the_start_variable_opens_one_page_over_the_list() {
        assert_eq!(start_routes(None), [Route::Agents]);
        assert_eq!(start_routes(Some("inbox")), [Route::Agents, Route::Inbox]);
        assert_eq!(
            start_routes(Some("agent:w1:p2")),
            [Route::Agents, Route::Agent("w1:p2".into())]
        );
        for ignored in ["", "agent:", "agents", "settings", "agent"] {
            assert_eq!(start_routes(Some(ignored)), [Route::Agents], "{ignored}");
        }
    }
}
