//! Access to the RDA archives of No Man's Land ("Resource File V1.0").

mod archive;
mod error;
mod gamedir;
#[doc(hidden)]
pub mod testutil;
mod version;
mod vfs;

pub use archive::{Archive, Entry};
pub use error::RdaError;
pub use gamedir::{
    Candidate, Found, REMEMBERED_FILE, Rejection, SearchInput, Source, Strictness, Tried, USER_DIR,
    candidates, check_candidate, check_choice, find_game_dir, read_remembered, remembered_file,
    remembered_file_from, search, write_remembered,
};
pub use version::{GameVersion, detect_version};
pub use vfs::{Vfs, normalize_path};
