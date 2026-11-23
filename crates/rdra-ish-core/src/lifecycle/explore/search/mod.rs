//! State admission, birth-history quotienting, views, and diagnostics.

mod bfs;
mod history;
mod views;
mod violations;

pub(super) use history::BirthHistory;
