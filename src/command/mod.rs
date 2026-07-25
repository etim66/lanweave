//! Slash command registry and command list UI.
//!
//! Defines [`CommandSpec`] entries such as `/help`, `/devices`, `/send`,
//! `/disconnect`, and `/quit`. The list shown when the user enters `/` is
//! filtered by the current [`app`] state.
