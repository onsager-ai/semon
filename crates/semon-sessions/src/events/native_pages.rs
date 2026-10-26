//! Content-free native page encoding shared by physical storage adapters.
//! These plans carry no Raw access or Catalog publication authority.
pub(crate) mod chunks;
pub(crate) mod launch;

pub(crate) struct Plan {
    pub(crate) header: String,
    pub(crate) pages: Vec<String>,
    pub(crate) tree: Vec<Vec<String>>,
}
