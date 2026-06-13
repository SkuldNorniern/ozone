use std::collections::HashMap;
use std::sync::Arc;

use ozone_buffer::{Buffer, BufferId};
use ozone_editor::buffer_language;
use ozone_syntax::{SyntaxFeatures, TokenSpan, fold_line_ranges, parse_features, scan_buffer};
use taste::Language;

/// Per-buffer syntax state: detected language, the structural parse result,
/// and the highlight/fold spans derived from it. All four are recomputed
/// together when the buffer's revision changes, replacing separate
/// highlight/fold caches that each re-detected the language and rescanned the
/// buffer independently.
#[derive(Default)]
pub(crate) struct BufferSyntaxState {
    revision: Option<u64>,
    pub language: Option<Language>,
    pub features: Option<Arc<SyntaxFeatures>>,
    pub highlights: Vec<Vec<TokenSpan>>,
    pub folds: Vec<(usize, usize)>,
}

impl BufferSyntaxState {
    /// Recompute language, structural features, highlights, and folds if
    /// `buf`'s revision changed since the last call. No-op otherwise.
    pub fn refresh(&mut self, buf: &Buffer) {
        let revision = buf.revision();
        if self.revision == Some(revision) {
            return;
        }
        self.revision = Some(revision);
        let language = buffer_language(buf);
        let (highlights, folds, features) = buf.with_text(|text| {
            (
                scan_buffer(language, text),
                fold_line_ranges(language, text),
                parse_features(language, text),
            )
        });
        self.highlights = highlights;
        self.folds = folds;
        self.features = features;
        self.language = language;
    }
}

/// Per-buffer syntax state cache, keyed by buffer id.
pub(crate) type SyntaxCache = HashMap<BufferId, BufferSyntaxState>;
