#[derive(Debug)]
pub(crate) struct Index {
    /// Fields included in the index.
    pub(crate) fields: Vec<IndexField>,

    /// When `true`, indexed entries are unique
    pub(crate) unique: bool,

    /// True when the index is the primary key
    pub(crate) primary_key: bool,

    /// User-provided index name from `#[index(name = "...", ...)]` or
    /// `#[key(name = "...", ...)]`. When `None`, the schema builder generates
    /// a name automatically.
    pub(crate) name: Option<String>,

    /// The predicate of a partial index from `filter = "..."`. `None` for an
    /// index over every row.
    pub(crate) filter: Option<String>,
}

impl Index {
    /// A partial index holds only the rows its filter admits, so a query
    /// without that predicate cannot use it, and a partial unique index does
    /// not make its fields unique across the table.
    pub(crate) fn is_partial(&self) -> bool {
        self.filter.is_some()
    }
}

#[derive(Debug)]
pub(crate) struct IndexField {
    /// The field being indexed
    pub(crate) field: usize,

    /// The scope of the index
    pub(crate) scope: IndexScope,
}

#[derive(Debug)]
pub(crate) enum IndexScope {
    /// The index column is used to partition rows across nodes of a distributed database.
    Partition,

    /// The index column is scoped to a physical node.
    Local,
}
