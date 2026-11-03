//! PDF pages and text for the viewer (P2 "PDF highlights"). On macOS this is PDFKit:
//! pages render into a CoreGraphics bitmap that is cached as PNG beside the blobs, and
//! the text under a highlight rectangle comes from PDFKit's selection. Elsewhere PDFs
//! open in the system viewer.

use std::path::{Path, PathBuf};

/// Width, in points, of a rendered page column; height follows the page's aspect.
pub const PAGE_WIDTH: f32 = 720.;

/// A minimal valid PDF with one page per entry, each line set in Helvetica. Used by
/// tests and demos; real PDFs come from users.
pub fn sample_pdf(pages: &[&[&str]]) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    let n = pages.len();
    // 1: catalog, 2: pages, 3: font, then (page, contents) pairs.
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {n} >>",
        kids.join(" ")
    ));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, lines) in pages.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
             /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + 2 * i
        ));
        let mut stream = String::from("BT /F1 20 Tf 72 720 Td 28 TL\n");
        for l in lines.iter() {
            let esc = l
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            stream.push_str(&format!("({esc}) Tj T*\n"));
        }
        stream.push_str("ET");
        objs.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objs.len() + 1
    ));
    for off in offsets {
        out.push_str(&format!("{off:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    ));
    out.into_bytes()
}

/// Decode a page PNG into a GPUI image (BGRA), synchronously: drawing the page never
/// waits on an asynchronous image load, so it appears on the first frame.
#[cfg(target_os = "macos")]
pub fn load_page_image(path: &Path) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(info.width, info.height, buf)?;
    Some(std::sync::Arc::new(gpui::RenderImage::new([
        image::Frame::new(img),
    ])))
}

/// Cached PNG for a rendered page.
pub fn page_cache_path(blob_dir: &Path, hash: &str, page: usize, scale: u32) -> PathBuf {
    blob_dir
        .join(".cache")
        .join(format!("{hash}-p{page}-x{scale}.png"))
}

#[cfg(target_os = "macos")]
pub use mac::PdfDoc;

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2's msg_send! checks a cfg this crate does not declare
mod mac {
    use std::ffi::{c_void, CStr};
    use std::path::Path;

    use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use foreign_types::ForeignType;
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}

    /// `kPDFDisplayBoxMediaBox`
    const MEDIA_BOX: isize = 0;
    const UTF8: usize = 4;

    /// An open PDF. PDFKit objects are used from the main thread only.
    pub struct PdfDoc {
        doc: *mut Object,
    }

    unsafe fn ns_string(s: &str) -> *mut Object {
        let cls = Class::get("NSString").expect("Foundation");
        let obj: *mut Object = msg_send![cls, alloc];
        msg_send![obj, initWithBytes: s.as_ptr() as *const c_void length: s.len() encoding: UTF8]
    }

    unsafe fn rust_string(ns: *mut Object) -> String {
        if ns.is_null() {
            return String::new();
        }
        let p: *const std::os::raw::c_char = msg_send![ns, UTF8String];
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }

    impl PdfDoc {
        pub fn open(path: &Path) -> Option<Self> {
            let cls = Class::get("PDFDocument")?;
            // SAFETY: standard Foundation/PDFKit constructors; ownership of the
            // document (+1 from alloc/init) is released in Drop.
            unsafe {
                let s = ns_string(&path.to_string_lossy());
                let url: *mut Object = msg_send![Class::get("NSURL")?, fileURLWithPath: s];
                let _: () = msg_send![s, release];
                let doc: *mut Object = msg_send![cls, alloc];
                let doc: *mut Object = msg_send![doc, initWithURL: url];
                (!doc.is_null()).then(|| Self { doc })
            }
        }

        pub fn page_count(&self) -> usize {
            // SAFETY: getter on a live PDFDocument.
            let n: usize = unsafe { msg_send![self.doc, pageCount] };
            n
        }

        fn page(&self, i: usize) -> Option<*mut Object> {
            if i >= self.page_count() {
                return None;
            }
            // SAFETY: index checked above; the page is owned by the document.
            let p: *mut Object = unsafe { msg_send![self.doc, pageAtIndex: i] };
            (!p.is_null()).then_some(p)
        }

        fn bounds(&self, page: *mut Object) -> CGRect {
            // SAFETY: getter returning an NSRect by value.
            unsafe { msg_send![page, boundsForBox: MEDIA_BOX] }
        }

        /// Page size in points.
        pub fn page_size(&self, i: usize) -> Option<(f32, f32)> {
            let b = self.bounds(self.page(i)?);
            Some((b.size.width as f32, b.size.height as f32))
        }

        /// Render a page `scale` pixels per point and write it as PNG.
        pub fn render_png(&self, i: usize, scale: f32, out: &Path) -> std::io::Result<()> {
            let page = self
                .page(i)
                .ok_or_else(|| std::io::Error::other("no such page"))?;
            let b = self.bounds(page);
            let w = (b.size.width as f32 * scale).ceil().max(1.) as usize;
            let h = (b.size.height as f32 * scale).ceil().max(1.) as usize;
            let mut ctx = CGContext::create_bitmap_context(
                None,
                w,
                h,
                8,
                0,
                &CGColorSpace::create_device_rgb(),
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            );
            ctx.set_rgb_fill_color(1., 1., 1., 1.);
            ctx.fill_rect(CGRect::new(
                &CGPoint::new(0., 0.),
                &CGSize::new(w as f64, h as f64),
            ));
            ctx.scale(scale as f64, scale as f64);
            ctx.translate(-b.origin.x, -b.origin.y);
            // SAFETY: drawing into a live bitmap context on this thread.
            unsafe {
                let _: () =
                    msg_send![page, drawWithBox: MEDIA_BOX toContext: ctx.as_ptr() as *mut c_void];
            }
            let stride = ctx.bytes_per_row();
            let data = ctx.data();
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                rgba.extend_from_slice(&data[y * stride..y * stride + w * 4]);
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = out.with_extension("png.tmp");
            let file = std::fs::File::create(&tmp)?;
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .and_then(|mut wr| wr.write_image_data(&rgba))
                .map_err(std::io::Error::other)?;
            std::fs::rename(tmp, out)
        }

        /// Page-relative (`0..1`, top-left origin) to PDF space (points, bottom-left).
        fn to_pdf(&self, page: *mut Object, p: (f32, f32)) -> CGPoint {
            let b = self.bounds(page);
            CGPoint::new(
                b.origin.x + p.0 as f64 * b.size.width,
                b.origin.y + (1. - p.1 as f64) * b.size.height,
            )
        }

        /// Text selected by dragging from `from` to `to` (page-relative points), the way a
        /// PDF reader selects: in reading order, line by line. Returns the text and one
        /// page-relative rectangle per line, or `None` when no text lies between them.
        pub fn select_text(
            &self,
            i: usize,
            from: (f32, f32),
            to: (f32, f32),
        ) -> Option<(String, Vec<[f32; 4]>)> {
            let page = self.page(i)?;
            let b = self.bounds(page);
            let (a, z) = (self.to_pdf(page, from), self.to_pdf(page, to));
            // SAFETY: PDFKit getters on live objects; results are autoreleased and read
            // before returning.
            unsafe {
                let sel: *mut Object = msg_send![page, selectionFromPoint: a toPoint: z];
                if sel.is_null() {
                    return None;
                }
                let s: *mut Object = msg_send![sel, string];
                let text = rust_string(s);
                if text.trim().is_empty() {
                    return None;
                }
                let lines: *mut Object = msg_send![sel, selectionsByLine];
                let n: usize = if lines.is_null() {
                    0
                } else {
                    msg_send![lines, count]
                };
                let mut rects = Vec::new();
                for k in 0..n {
                    let line: *mut Object = msg_send![lines, objectAtIndex: k];
                    let r: CGRect = msg_send![line, boundsForPage: page];
                    if r.size.width <= 0. || r.size.height <= 0. {
                        continue;
                    }
                    let (pw, ph) = (b.size.width, b.size.height);
                    let x = ((r.origin.x - b.origin.x) / pw) as f32;
                    let top = ((b.origin.y + ph - (r.origin.y + r.size.height)) / ph) as f32;
                    rects.push([
                        x.clamp(0., 1.),
                        top.clamp(0., 1.),
                        ((r.size.width / pw) as f32).clamp(0., 1.),
                        ((r.size.height / ph) as f32).clamp(0., 1.),
                    ]);
                }
                // PDFKit snaps a selection to the nearest text even from empty paper; keep
                // it only if some selected line actually lies under the drag.
                let (x0, x1) = (from.0.min(to.0) - 0.01, from.0.max(to.0) + 0.01);
                let (y0, y1) = (from.1.min(to.1) - 0.01, from.1.max(to.1) + 0.01);
                let touched = rects.iter().any(|r| {
                    r[1] <= y1
                        && r[1] + r[3] >= y0
                        && (rects.len() > 1 || (r[0] <= x1 && r[0] + r[2] >= x0))
                });
                (touched && !rects.is_empty()).then_some((text, rects))
            }
        }

        /// Text under a page-relative rectangle (`0..1`, origin top-left).
        pub fn text_in(&self, i: usize, rect: [f32; 4]) -> String {
            let Some(page) = self.page(i) else {
                return String::new();
            };
            let b = self.bounds(page);
            let (pw, ph) = (b.size.width, b.size.height);
            // PDF space has its origin at the bottom-left.
            let r = CGRect::new(
                &CGPoint::new(
                    b.origin.x + rect[0] as f64 * pw,
                    b.origin.y + (1. - rect[1] as f64 - rect[3] as f64) * ph,
                ),
                &CGSize::new(rect[2] as f64 * pw, rect[3] as f64 * ph),
            );
            // SAFETY: PDFSelection is autoreleased and read immediately.
            unsafe {
                let sel: *mut Object = msg_send![page, selectionForRect: r];
                if sel.is_null() {
                    return String::new();
                }
                let s: *mut Object = msg_send![sel, string];
                rust_string(s)
            }
        }
    }

    impl Drop for PdfDoc {
        fn drop(&mut self) {
            // SAFETY: balances the +1 from alloc/init in `open`.
            unsafe {
                let _: () = msg_send![self.doc, release];
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn renders_pages_and_reads_text() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.pdf");
        std::fs::write(
            &f,
            sample_pdf(&[&["Hello Trellis", "second line"], &["Page two text"]]),
        )
        .unwrap();
        let doc = PdfDoc::open(&f).expect("PDFKit opens it");
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.page_size(0), Some((612., 792.)));
        let out = page_cache_path(dir.path(), "h", 0, 1);
        doc.render_png(0, 1.0, &out).unwrap();
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&out).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (612, 792));
        assert!(buf.chunks(4).any(|p| p[0] < 100), "some dark text pixels");
        assert!(
            buf.chunks(4).filter(|p| p[0] > 250).count() > buf.len() / 8,
            "mostly white"
        );
        // The first line sits about 72pt below the top: y ≈ 0.07..0.11 of the page.
        let text = doc.text_in(0, [0.0, 0.05, 1.0, 0.06]);
        assert!(text.contains("Hello Trellis"), "{text:?}");
        assert!(!text.contains("Page two"));
        assert!(doc.text_in(1, [0., 0., 1., 1.]).contains("Page two text"));
        assert!(doc.text_in(7, [0., 0., 1., 1.]).is_empty());
        // Reader-style selection: from inside the first line to inside the second.
        let (text, rects) = doc.select_text(0, (0.13, 0.085), (0.25, 0.12)).unwrap();
        assert!(
            text.starts_with("Trellis") || text.contains("Trellis"),
            "{text:?}"
        );
        assert!(text.contains("second"), "{text:?}");
        assert_eq!(rects.len(), 2, "one rectangle per line: {rects:?}");
        assert!(rects[0][1] < rects[1][1], "top line first");
        assert!(rects.iter().all(|r| r[2] > 0. && r[3] > 0.01 && r[3] < 0.1));
        // Over empty paper there is nothing to select.
        assert!(doc.select_text(0, (0.1, 0.6), (0.9, 0.7)).is_none());
        assert!(PdfDoc::open(&dir.path().join("missing.pdf")).is_none());
    }
}
