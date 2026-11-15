//! **The Friends page** (the web app's *Amis*): what the account's Plex friends watched, the
//! account's "Share what I watch" setting and, for a server's owner, what plays on it now.
//!
//! * [`page`]: [`page::FriendsScreen`], its input alphabet [`page::FriendsIn`] and the store
//!   snapshot [`page::FriendsView`]; what it asks leaves on the shared lanes (`ContentReq`, and the
//!   community store's `StoreCmd::Community`).
//! * [`header`]: the title block and the one row of controls (the friend filter, the toggle).
//!
//! The rules (posts, days, relative times) are `plx_data::community`'s; the reads and the one write
//! are `plx_data::stores::community`'s. No sibling screen is named here.
pub mod header;
pub mod page;

#[cfg(test)]
mod page_tests;
