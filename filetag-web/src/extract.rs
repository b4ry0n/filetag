//! Pure-Rust embedded JPEG extraction from image containers.
//!
//! No external tools needed. Two public entry points:
//! - [`raw_embedded_jpeg`]  — TIFF-family RAW formats + Fujifilm RAF
//! - [`heic_extract_jpeg_thumbnail`] — HEIC/HEIF (ISOBMFF) thumbnail items

// ---------------------------------------------------------------------------
// Pure-Rust embedded JPEG extraction from RAW files
// ---------------------------------------------------------------------------

/// Read a `u16` from `data[off..]` with the given byte order.
fn tiff_u16(data: &[u8], off: usize, le: bool) -> Option<u16> {
    let s = data.get(off..off + 2)?;
    Some(if le {
        u16::from_le_bytes([s[0], s[1]])
    } else {
        u16::from_be_bytes([s[0], s[1]])
    })
}

/// Read a `u32` from `data[off..]` with the given byte order.
fn tiff_u32(data: &[u8], off: usize, le: bool) -> Option<u32> {
    let s = data.get(off..off + 4)?;
    let arr: [u8; 4] = s.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(arr)
    } else {
        u32::from_be_bytes(arr)
    })
}

/// Walk one TIFF IFD at `offset`, collecting `(jpeg_offset, jpeg_len)` pairs
/// and any sub-IFD offsets. Returns the next-IFD offset when non-zero.
fn tiff_walk_ifd(
    data: &[u8],
    offset: usize,
    le: bool,
    jpegs: &mut Vec<(usize, usize)>,
    sub_ifds: &mut Vec<usize>,
) -> Option<usize> {
    let count = tiff_u16(data, offset, le)? as usize;
    let base = offset + 2;
    // Ensure all entry bytes are in-bounds before iterating.
    data.get(base..base + count * 12)?;

    let mut jpeg_off: Option<usize> = None;
    let mut jpeg_len: Option<usize> = None;

    for i in 0..count {
        let e = base + i * 12;
        let tag = tiff_u16(data, e, le).unwrap_or(0);
        let typ = tiff_u16(data, e + 2, le).unwrap_or(0);
        let cnt = tiff_u32(data, e + 4, le).unwrap_or(0) as usize;

        match tag {
            // JPEGInterchangeFormat: file offset to embedded JPEG
            0x0201 => jpeg_off = tiff_u32(data, e + 8, le).map(|v| v as usize),
            // JPEGInterchangeFormatLength: byte length of embedded JPEG
            0x0202 => jpeg_len = tiff_u32(data, e + 8, le).map(|v| v as usize),
            // SubIFD: one or more additional IFD offsets (used in DNG / NEF)
            0x014A => {
                if cnt == 1 {
                    if let Some(v) = tiff_u32(data, e + 8, le) {
                        sub_ifds.push(v as usize);
                    }
                } else if typ == 4 {
                    // LONG array; value field is a pointer to the array
                    if let Some(arr_off) = tiff_u32(data, e + 8, le).map(|v| v as usize) {
                        for j in 0..cnt {
                            if let Some(v) = tiff_u32(data, arr_off + j * 4, le) {
                                sub_ifds.push(v as usize);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if let (Some(off), Some(len)) = (jpeg_off, jpeg_len)
        && len > 0
    {
        jpegs.push((off, len));
    }

    // The next-IFD offset is stored immediately after the last entry.
    let next_off = tiff_u32(data, base + count * 12, le).unwrap_or(0) as usize;
    if next_off != 0 { Some(next_off) } else { None }
}

/// Read the EXIF Orientation tag (0x0112) from TIFF IFD0.
/// Returns the raw orientation value (1–8) or 1 (no rotation) on failure.
pub fn raw_tiff_orientation(data: &[u8]) -> u8 {
    let le = match data.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return 1,
    };
    if tiff_u16(data, 2, le) != Some(42) {
        return 1;
    }
    let ifd0_off = match tiff_u32(data, 4, le) {
        Some(v) => v as usize,
        None => return 1,
    };
    let count = match tiff_u16(data, ifd0_off, le) {
        Some(v) => v as usize,
        None => return 1,
    };
    let base = ifd0_off + 2;
    if data.get(base..base + count * 12).is_none() {
        return 1;
    }
    for i in 0..count {
        let e = base + i * 12;
        let tag = tiff_u16(data, e, le).unwrap_or(0);
        if tag == 0x0112 {
            // Type is SHORT (3); value fits directly in the value field.
            let val = tiff_u16(data, e + 8, le).unwrap_or(1);
            return if (1..=8).contains(&val) { val as u8 } else { 1 };
        }
    }
    1
}

/// Extract an embedded JPEG preview from a TIFF-family RAW file.
/// Covers NEF, CR2, ARW, ORF, DNG, PEF, SRW, RW2 and most other TIFF-based
/// formats. Prefers the largest JPEG found (full preview over tiny thumbnail).
fn raw_embedded_jpeg_tiff(data: &[u8]) -> Option<Vec<u8>> {
    let le = match data.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    // Standard TIFF magic number = 42
    if tiff_u16(data, 2, le)? != 42 {
        return None;
    }
    let ifd0_off = tiff_u32(data, 4, le)? as usize;

    let mut jpegs: Vec<(usize, usize)> = Vec::new();
    let mut sub_ifds: Vec<usize> = Vec::new();

    // Walk IFD0 and the full linked IFD chain (IFD1, IFD2, …)
    let mut next = tiff_walk_ifd(data, ifd0_off, le, &mut jpegs, &mut sub_ifds);
    while let Some(off) = next {
        next = tiff_walk_ifd(data, off, le, &mut jpegs, &mut sub_ifds);
    }
    // Walk sub-IFDs (DNG preview IFD, NEF large preview, etc.)
    for off in sub_ifds {
        tiff_walk_ifd(data, off, le, &mut jpegs, &mut Vec::new());
    }

    // Pick the largest valid JPEG (most likely the full-resolution preview).
    jpegs.sort_by_key(|&(_, len)| std::cmp::Reverse(len));
    for (off, len) in jpegs {
        if let Some(slice) = data.get(off..off + len)
            && slice.starts_with(&[0xFF, 0xD8])
        {
            return Some(slice.to_vec());
        }
    }
    None
}

/// Extract an embedded JPEG preview from a Fujifilm RAF file.
/// RAF header (big-endian):
///   0x00–0x0F  "FUJIFILMCCD-RAW " (magic)
///   0x44–0x47  u32 preview image file offset
///   0x48–0x4B  u32 preview image size in bytes
fn raf_extract_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let off = u32::from_be_bytes(data.get(0x44..0x48)?.try_into().ok()?) as usize;
    let len = u32::from_be_bytes(data.get(0x48..0x4C)?.try_into().ok()?) as usize;
    if len == 0 {
        return None;
    }
    let slice = data.get(off..off + len)?;
    if slice.starts_with(&[0xFF, 0xD8]) {
        Some(slice.to_vec())
    } else {
        None
    }
}

/// Extract the largest embedded JPEG preview from a RAW file without any
/// external tools. Handles TIFF-family formats and Fujifilm RAF. Returns
/// `None` for unsupported container types (e.g. CR3 / ISOBMFF).
pub fn raw_embedded_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if data.starts_with(b"FUJIFILMCCD-RAW ") {
        return raf_extract_jpeg(data);
    }
    raw_embedded_jpeg_tiff(data)
}

// ---------------------------------------------------------------------------
// HEIC/HEIF — pure-Rust ISOBMFF thumbnail extractor
// ---------------------------------------------------------------------------

/// Read a 4-byte big-endian u32 from `data` at `off`. Returns `None` on OOB.
fn bmff_u32(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(off..off + 4)?.try_into().ok()?))
}

/// Iterate over ISOBMFF boxes in `data[0..data.len()]`, calling `cb(box_type, payload)`.
/// `box_type` is the 4-byte ASCII tag as `[u8; 4]`; `payload` excludes the 8-byte header.
/// 64-bit extended sizes are supported. Stops early when `cb` returns `true`.
fn bmff_iter<F>(data: &[u8], mut cb: F)
where
    F: FnMut([u8; 4], &[u8]) -> bool,
{
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size32 = bmff_u32(data, pos).unwrap_or(0) as usize;
        let tag: [u8; 4] = match data.get(pos + 4..pos + 8) {
            Some(b) => b.try_into().unwrap(),
            None => break,
        };
        let (header_len, box_len) = if size32 == 1 {
            // 64-bit extended size stored in the next 8 bytes.
            if pos + 16 > data.len() {
                break;
            }
            let hi = bmff_u32(data, pos + 8).unwrap_or(0) as u64;
            let lo = bmff_u32(data, pos + 12).unwrap_or(0) as u64;
            let ext = ((hi << 32) | lo) as usize;
            (16, ext)
        } else if size32 == 0 {
            // Box extends to end of file.
            (8, data.len() - pos)
        } else {
            (8, size32)
        };
        if box_len < header_len || pos + box_len > data.len() {
            break;
        }
        let payload = &data[pos + header_len..pos + box_len];
        if cb(tag, payload) {
            return;
        }
        pos += box_len;
    }
}

/// Item location entry from the HEIC `iloc` box.
struct IlocEntry {
    item_id: u16,
    offset: usize,
    length: usize,
}

/// Parse a HEIC `iloc` box (versions 0 and 1).
fn parse_iloc(data: &[u8]) -> Vec<IlocEntry> {
    let mut entries = Vec::new();
    if data.len() < 8 {
        return entries;
    }
    // iloc: version(1) + flags(3) + offset_size(4bits)|length_size(4bits)
    //       + base_offset_size(4bits)|reserved(4bits) + item_count(2)
    let version = data[0];
    if version > 1 {
        return entries; // only v0/v1 needed for thumbnails
    }
    let offset_size = ((data[4] >> 4) & 0x0F) as usize;
    let length_size = (data[4] & 0x0F) as usize;
    let base_offset_size = ((data[5] >> 4) & 0x0F) as usize;
    let item_count = u16::from_be_bytes([data[6], data[7]]) as usize;
    let mut pos = 8usize;

    let read_uint = |buf: &[u8], p: usize, sz: usize| -> usize {
        match sz {
            2 => u16::from_be_bytes(
                buf.get(p..p + 2)
                    .and_then(|b| b.try_into().ok())
                    .unwrap_or([0; 2]),
            ) as usize,
            4 => u32::from_be_bytes(
                buf.get(p..p + 4)
                    .and_then(|b| b.try_into().ok())
                    .unwrap_or([0; 4]),
            ) as usize,
            8 => u64::from_be_bytes(
                buf.get(p..p + 8)
                    .and_then(|b| b.try_into().ok())
                    .unwrap_or([0; 8]),
            ) as usize,
            _ => 0,
        }
    };

    for _ in 0..item_count {
        if pos + 2 > data.len() {
            break;
        }
        let item_id = u16::from_be_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        if version == 1 {
            pos += 2; // construction_method u16
        }
        pos += 2; // data_reference_index
        let base_offset = read_uint(data, pos, base_offset_size);
        pos += base_offset_size;
        if pos + 2 > data.len() {
            break;
        }
        let extent_count = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        // Read only the first extent; skip the rest.
        if extent_count > 0 && pos + offset_size + length_size <= data.len() {
            let ext_offset = read_uint(data, pos, offset_size);
            let ext_length = read_uint(data, pos + offset_size, length_size);
            entries.push(IlocEntry {
                item_id,
                offset: base_offset + ext_offset,
                length: ext_length,
            });
        }
        pos += extent_count * (offset_size + length_size);
    }
    entries
}

/// Parse a HEIC `iinf` box; returns `[(item_id, item_type_4cc)]`.
fn parse_iinf(data: &[u8]) -> Vec<(u16, [u8; 4])> {
    let mut items = Vec::new();
    if data.len() < 6 {
        return items;
    }
    // iinf: version(1) + flags(3) + entry_count(2)
    let entry_count = u16::from_be_bytes([data[4], data[5]]) as usize;
    let mut pos = 6usize;
    for _ in 0..entry_count {
        if pos + 8 > data.len() {
            break;
        }
        let box_size = bmff_u32(data, pos).unwrap_or(0) as usize;
        let box_tag: [u8; 4] = match data.get(pos + 4..pos + 8) {
            Some(b) => b.try_into().unwrap(),
            None => break,
        };
        if box_size < 8 || pos + box_size > data.len() {
            break;
        }
        if &box_tag == b"infe" {
            let infe = &data[pos + 8..pos + box_size];
            // infe v2+: version(1) + flags(3) + item_ID(2) + item_protection_index(2) + item_type(4)
            if infe.len() >= 12 && infe[0] >= 2 {
                let item_id = u16::from_be_bytes([infe[4], infe[5]]);
                let item_type: [u8; 4] = infe[8..12].try_into().unwrap_or([0; 4]);
                items.push((item_id, item_type));
            }
        }
        pos += box_size;
    }
    items
}

/// Parse the `iref` box; returns item_ids that appear as `thmb` (thumbnail) sources.
fn parse_iref_thumbs(data: &[u8]) -> Vec<u16> {
    let mut thumb_ids = Vec::new();
    if data.len() < 4 {
        return thumb_ids;
    }
    // iref: version(1) + flags(3) + list of SingleItemTypeReferenceBox entries
    let mut pos = 4usize;
    while pos + 8 <= data.len() {
        let box_size = bmff_u32(data, pos).unwrap_or(0) as usize;
        let box_tag: [u8; 4] = match data.get(pos + 4..pos + 8) {
            Some(b) => b.try_into().unwrap(),
            None => break,
        };
        if box_size < 12 || pos + box_size > data.len() {
            break;
        }
        if &box_tag == b"thmb" {
            // thmb: from_item_id(2) + reference_count(2) + to_items(2 each).
            // Collect from_item_id — that is the thumbnail item.
            let from_id = u16::from_be_bytes([data[pos + 8], data[pos + 9]]);
            thumb_ids.push(from_id);
        }
        pos += box_size;
    }
    thumb_ids
}

/// Try to extract an embedded JPEG thumbnail from a HEIC/HEIF file using
/// pure-Rust ISOBMFF parsing. No external tools needed.
///
/// Parses the `meta` → `iinf` / `iloc` / `iref` hierarchy.  Prefers items
/// with `item_type == jpeg`, falling back to `thmb`-referenced items.
pub fn heic_extract_jpeg_thumbnail(data: &[u8]) -> Option<Vec<u8>> {
    // Confirm HEIC/HEIF family via ftyp brand.
    let mut is_heic = false;
    bmff_iter(data, |tag, payload| {
        if &tag == b"ftyp" {
            if let Some(brand) = payload.get(0..4) {
                is_heic = matches!(brand, b"heic" | b"heif" | b"heix" | b"mif1" | b"msf1");
            }
            true
        } else {
            false
        }
    });
    if !is_heic {
        return None;
    }

    // Find the top-level meta box (Apple HEIC keeps it top-level).
    let mut meta_payload: Option<Vec<u8>> = None;
    bmff_iter(data, |tag, payload| {
        if &tag == b"meta" {
            meta_payload = Some(payload.to_vec());
            true
        } else {
            false
        }
    });
    // meta is a FullBox: skip 4-byte version+flags before children.
    let meta = meta_payload?.get(4..)?.to_vec();

    let mut iinf_items: Vec<(u16, [u8; 4])> = Vec::new();
    let mut iloc_entries: Vec<IlocEntry> = Vec::new();
    let mut thumb_ids: Vec<u16> = Vec::new();

    bmff_iter(&meta, |tag, payload| {
        match &tag {
            b"iinf" => {
                iinf_items = parse_iinf(payload);
            }
            b"iloc" => {
                iloc_entries = parse_iloc(payload);
            }
            b"iref" => {
                thumb_ids = parse_iref_thumbs(payload);
            }
            _ => {}
        }
        false
    });

    // Prefer explicit jpeg items; fall back to thumbnail-referenced items.
    let jpeg_item_ids: Vec<u16> = iinf_items
        .iter()
        .filter(|(_, t)| t == b"jpeg" || t == b"JPEG")
        .map(|(id, _)| *id)
        .collect();

    let candidates: Vec<u16> = if !jpeg_item_ids.is_empty() {
        jpeg_item_ids
    } else if !thumb_ids.is_empty() {
        thumb_ids
    } else {
        return None;
    };

    for item_id in candidates {
        if let Some(iloc) = iloc_entries.iter().find(|e| e.item_id == item_id)
            && iloc.length > 0
        {
            let raw = data.get(iloc.offset..iloc.offset + iloc.length)?;
            if raw.starts_with(&[0xFF, 0xD8]) {
                return Some(raw.to_vec());
            }
        }
    }
    None
}
