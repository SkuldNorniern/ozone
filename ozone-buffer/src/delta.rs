/// A single reversible edit applied to a piece table.
#[derive(Debug, Clone)]
pub struct Delta {
    pub kind: DeltaKind,
}

#[derive(Debug, Clone)]
pub enum DeltaKind {
    Insert { offset: usize, text: String },
    Delete { offset: usize, text: String },
}
