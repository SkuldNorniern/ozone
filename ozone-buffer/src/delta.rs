/// A single reversible edit applied to a piece table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub kind: DeltaKind,
}

impl Delta {
    /// The edit that reverses this delta.
    pub fn inverse(&self) -> Self {
        let kind = match &self.kind {
            DeltaKind::Insert { offset, text } => DeltaKind::Delete {
                offset: *offset,
                text: text.clone(),
            },
            DeltaKind::Delete { offset, text } => DeltaKind::Insert {
                offset: *offset,
                text: text.clone(),
            },
        };
        Self { kind }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaKind {
    Insert { offset: usize, text: String },
    Delete { offset: usize, text: String },
}
