//! Vision OCR fallback wiring (F2 milestone 5).
//!
//! Resolves the [`ImageTextExtractor`] the harness uses to turn attached
//! images into text when they can't be sent to a vision-capable model. The
//! concrete Tesseract backend is behind the optional `ocr-tesseract` build
//! feature (a native `libtesseract` dependency), so a default build links no
//! OCR engine and `ocr = "tesseract"` is reported as unavailable.

use std::sync::Arc;

use peridot_common::OcrBackend;
use peridot_core::ImageTextExtractor;

/// Builds the OCR extractor selected by `[vision] ocr`, or `None` when OCR is
/// off or the requested backend wasn't compiled in. Logs a warning when a
/// backend is requested but unavailable so the misconfiguration is visible.
pub fn ocr_extractor(backend: OcrBackend) -> Option<Arc<dyn ImageTextExtractor>> {
    match backend {
        OcrBackend::Off => None,
        OcrBackend::Tesseract => tesseract_extractor(),
    }
}

#[cfg(feature = "ocr-tesseract")]
fn tesseract_extractor() -> Option<Arc<dyn ImageTextExtractor>> {
    Some(Arc::new(tesseract::TesseractExtractor::new("eng")))
}

#[cfg(not(feature = "ocr-tesseract"))]
fn tesseract_extractor() -> Option<Arc<dyn ImageTextExtractor>> {
    eprintln!(
        "warning: [vision] ocr = \"tesseract\" requested, but this build was \
compiled without the `ocr-tesseract` feature; attached images on a text-only \
model will be dropped to their placeholder. Rebuild with \
`--features ocr-tesseract` (requires libtesseract) to enable OCR."
    );
    None
}

#[cfg(feature = "ocr-tesseract")]
mod tesseract {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use peridot_core::ImageTextExtractor;
    use peridot_llm::ImageContent;
    use std::sync::Mutex;

    /// OCR backend backed by Tesseract via `leptess`. The `LepTess` handle is
    /// not `Sync`, so it is guarded by a mutex; OCR is invoked at most once per
    /// image on the turn-assembly path, so contention is negligible.
    pub struct TesseractExtractor {
        engine: Mutex<leptess::LepTess>,
    }

    impl TesseractExtractor {
        /// Creates an extractor for the given Tesseract language (e.g. `"eng"`).
        /// Falls back to a lazily-failing handle if the language data is
        /// missing — extraction then simply returns `None`.
        pub fn new(lang: &str) -> Self {
            let engine = leptess::LepTess::new(None, lang)
                .expect("tesseract language data should be installed for OCR");
            Self {
                engine: Mutex::new(engine),
            }
        }
    }

    impl ImageTextExtractor for TesseractExtractor {
        fn extract(&self, image: &ImageContent) -> Option<String> {
            let bytes = STANDARD.decode(&image.data).ok()?;
            let mut engine = self.engine.lock().ok()?;
            engine.set_image_from_mem(&bytes).ok()?;
            let text = engine.get_utf8_text().ok()?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    }
}
