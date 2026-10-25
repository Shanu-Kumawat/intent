//! intent-core: the single source of truth for Intent's task model, storage,
//! lifecycle, and views. Both the CLI and the GUI are thin layers over this
//! crate — no business logic lives anywhere else (SPEC §6).

pub mod dates;
pub mod error;
pub mod id;
pub mod model;
pub mod ops;
pub mod store;
pub mod views;

pub use dates::{parse_date, today, Date};
pub use error::{CoreError, Result};
pub use model::{NewTask, Patch, Priority, Status, Task, TaskPatch, TaskView, ALL_VIEWS};
pub use ops::{apply_change, RowChange};
pub use store::Store;
pub use views::{for_view, search, counts, ListFilter};
