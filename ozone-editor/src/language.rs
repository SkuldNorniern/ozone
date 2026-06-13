use std::path::Path;

use ozone_buffer::{Buffer, BufferKind};
use taste::{Language, detect_buffer};

/// Detect the language of a live editor buffer.
///
/// File buffers use both their path and current contents, allowing shebangs to
/// identify extensionless scripts. Scratch buffers have no path hint but may
/// still be identified by a shebang. Other virtual buffers are editor surfaces,
/// not source documents.
pub fn buffer_language(buffer: &Buffer) -> Option<Language> {
    let detection = buffer.with_text(|text| match &buffer.kind {
        BufferKind::File(path) => detect_buffer(Some(path.as_path()), text.as_bytes()),
        BufferKind::Scratch => detect_buffer::<&Path>(None, text.as_bytes()),
        _ => None,
    });
    detection.map(|result| result.language)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ozone_buffer::BufferKind;

    use super::*;

    #[test]
    fn extensionless_file_uses_shebang() {
        let buffer = Buffer::virtual_buffer(
            BufferKind::File(PathBuf::from("script")),
            "#!/usr/bin/env python\nprint('hi')\n",
        );
        assert_eq!(buffer_language(&buffer), Some(Language::PYTHON));
    }

    #[test]
    fn extension_still_detects_language() {
        let buffer = Buffer::virtual_buffer(BufferKind::File(PathBuf::from("main.rs")), "");
        assert_eq!(buffer_language(&buffer), Some(Language::RUST));
    }

    #[test]
    fn scratch_buffer_can_use_shebang() {
        let buffer = Buffer::from_text("#!/bin/bash\necho hi\n");
        assert_eq!(buffer_language(&buffer), Some(Language::SHELL));
    }

    #[test]
    fn editor_virtual_buffer_has_no_language() {
        let buffer = Buffer::virtual_buffer(BufferKind::Search, "#!/bin/bash\n");
        assert_eq!(buffer_language(&buffer), None);
    }
}
