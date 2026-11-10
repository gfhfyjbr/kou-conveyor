// Match the four registered Go DecodeConfig paths, not merely their size fields.
// Invalid headers use the same conservative token estimate as unknown images.
use std::io::{self, BufRead, Read};

pub(super) fn dimensions(reader: &mut impl BufRead) -> Option<(u32, u32)> {
    let magic = reader.fill_buf().ok()?;
    if magic.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(reader)
    } else if magic.starts_with(b"GIF87a") || magic.starts_with(b"GIF89a") {
        gif(reader)
    } else if magic.starts_with(b"\xff\xd8\xff") {
        jpeg(reader)
    } else if magic.starts_with(b"RIFF") && magic.get(8..12) == Some(b"WEBP") {
        webp(reader)
    } else {
        None
    }
}
fn read<const N: usize>(reader: &mut impl Read) -> Option<[u8; N]> {
    let mut data = [0; N];
    reader.read_exact(&mut data).ok()?;
    Some(data)
}
fn skip(reader: &mut impl Read, size: u64) -> Option<()> {
    let copied = io::copy(&mut reader.take(size), &mut io::sink()).ok()?;
    (copied == size).then_some(())
}
fn be32(data: &[u8]) -> u32 {
    u32::from_be_bytes(data.try_into().unwrap())
}
fn le32(data: &[u8]) -> u32 {
    u32::from_le_bytes(data.try_into().unwrap())
}
fn be16(data: &[u8]) -> u16 {
    u16::from_be_bytes(data.try_into().unwrap())
}
fn le16(data: &[u8]) -> u16 {
    u16::from_le_bytes(data.try_into().unwrap())
}

fn png(reader: &mut impl Read) -> Option<(u32, u32)> {
    read::<8>(reader)?;
    let (mut width, mut height, mut depth, mut color, mut stage) = (0, 0, 0, 0, 0);
    loop {
        let header = read::<8>(reader)?;
        let size = be32(&header[..4]);
        let kind = &header[4..];
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        let mut body = [0; 768];
        match kind {
            b"IHDR" => {
                if stage != 0 || size != 13 {
                    return None;
                }
                reader.read_exact(&mut body[..13]).ok()?;
                crc.update(&body[..13]);
                width = be32(&body[..4]);
                height = be32(&body[4..8]);
                depth = body[8];
                color = body[9];
                if body[10] != 0
                    || body[11] != 0
                    || body[12] > 1
                    || width == 0
                    || height == 0
                    || width > i32::MAX as u32
                    || height > i32::MAX as u32
                    || u64::from(width) * u64::from(height) > i64::MAX as u64 / 8
                {
                    return None;
                }
                if !matches!(
                    (depth, color),
                    (1 | 2 | 4, 0 | 3) | (8, 0 | 2 | 3 | 4 | 6) | (16, 0 | 2 | 4 | 6)
                ) {
                    return None;
                }
                stage = 1;
            }
            b"PLTE" => {
                let entries = size / 3;
                if stage != 1
                    || !size.is_multiple_of(3)
                    || entries == 0
                    || entries > 256
                    || entries > 1u32 << depth
                    || !matches!(color, 2 | 3 | 6)
                {
                    return None;
                }
                reader.read_exact(&mut body[..size as usize]).ok()?;
                crc.update(&body[..size as usize]);
                stage = 2;
            }
            b"tRNS" => {
                if stage != 2 || color != 3 || size > 256 {
                    return None;
                }
                reader.read_exact(&mut body[..size as usize]).ok()?;
                crc.update(&body[..size as usize]);
                stage = 3;
            }
            b"IDAT" => {
                if stage < 2 {
                    return None;
                }
                return Some((width, height));
            }
            b"IEND" => return None,
            _ => {
                if size > i32::MAX as u32 {
                    return None;
                }
                let mut remaining = size as usize;
                while remaining != 0 {
                    let size = remaining.min(body.len());
                    reader.read_exact(&mut body[..size]).ok()?;
                    crc.update(&body[..size]);
                    remaining -= size;
                }
            }
        }
        if be32(&read::<4>(reader)?) != crc.finalize() {
            return None;
        }
        if stage > 0 && color != 3 || stage >= 3 {
            return Some((width, height));
        }
    }
}
fn gif(reader: &mut impl Read) -> Option<(u32, u32)> {
    let header = read::<13>(reader)?;
    if header[10] & 0x80 != 0 {
        skip(reader, 3 << (1 + (header[10] & 7)))?;
    }
    Some((
        u32::from(le16(&header[6..8])),
        u32::from(le16(&header[8..10])),
    ))
}
fn jpeg(reader: &mut impl Read) -> Option<(u32, u32)> {
    read::<2>(reader)?;
    let (mut width, mut height) = (0, 0);
    let mut frame = false;
    let mut jfif = false;
    loop {
        let mut marker = read::<2>(reader)?;
        while marker[0] != 0xff {
            marker[0] = marker[1];
            marker[1] = read::<1>(reader)?[0];
        }
        if marker[1] == 0 {
            continue;
        }
        while marker[1] == 0xff {
            marker[1] = read::<1>(reader)?[0];
        }
        if marker[1] == 0xd9 {
            return None;
        }
        if (0xd0..=0xd7).contains(&marker[1]) {
            continue;
        }
        let size = be16(&read::<2>(reader)?) as usize;
        if size < 2 {
            return None;
        }
        let size = size - 2;
        match marker[1] {
            0xc0..=0xc2 => {
                if frame || !matches!(size, 9 | 15 | 18) {
                    return None;
                }
                let mut data = [0; 18];
                reader.read_exact(&mut data[..size]).ok()?;
                let components = (size - 6) / 3;
                if data[0] != 8 || data[5] as usize != components {
                    return None;
                }
                let mut ratios = [(0, 0); 4];
                for index in 0..components {
                    let id = data[6 + 3 * index];
                    if (0..index).any(|old| data[6 + 3 * old] == id) || data[8 + 3 * index] > 3 {
                        return None;
                    }
                    let hv = data[7 + 3 * index];
                    let h = hv >> 4;
                    let v = hv & 15;
                    if !matches!(h, 1 | 2 | 4) || !matches!(v, 1 | 2 | 4) {
                        return None;
                    }
                    if components == 4
                        && match index {
                            0 => !matches!(hv, 0x11 | 0x22),
                            1 | 2 => hv != 0x11,
                            3 => (h, v) != ratios[0],
                            _ => false,
                        }
                    {
                        return None;
                    }
                    ratios[index] = (h, v);
                }
                if components == 3 {
                    let max_h = ratios[..3].iter().map(|(h, _)| *h).max()?;
                    let max_v = ratios[..3].iter().map(|(_, v)| *v).max()?;
                    if ratios[..3]
                        .iter()
                        .any(|(h, v)| max_h % h != 0 || max_v % v != 0)
                    {
                        return None;
                    }
                }
                width = u32::from(be16(&data[3..5]));
                height = u32::from(be16(&data[1..3]));
                frame = true;
                if jfif {
                    return Some((width, height));
                }
            }
            0xda => return Some((width, height)),
            0xe0 if size >= 5 => {
                jfif = &read::<5>(reader)? == b"JFIF\0";
                skip(reader, (size - 5) as u64)?;
            }
            0xc4 | 0xdb | 0xdd | 0xe0..=0xef | 0xfe => {
                skip(reader, size as u64)?;
            }
            _ => return None,
        }
    }
}
fn webp(reader: &mut impl Read) -> Option<(u32, u32)> {
    let header = read::<12>(reader)?;
    let mut remaining = le32(&header[4..8]).checked_sub(4)?;
    loop {
        remaining = remaining.checked_sub(8)?;
        let header = read::<8>(reader)?;
        let size = le32(&header[4..8]);
        if size > remaining {
            return None;
        }
        match &header[..4] {
            b"VP8X" => {
                if size != 10 {
                    return None;
                }
                let data = read::<10>(reader)?;
                let width = 1 + u32::from_le_bytes([data[4], data[5], data[6], 0]);
                let height = 1 + u32::from_le_bytes([data[7], data[8], data[9], 0]);
                return (u64::from(width) * u64::from(height) <= i32::MAX as u64)
                    .then_some((width, height));
            }
            b"VP8L" => {
                if size < 5 {
                    return None;
                }
                let data = read::<5>(reader)?;
                if data[0] != 0x2f || data[4] >> 5 != 0 {
                    return None;
                }
                let bits = le32(&data[1..5]);
                return Some((1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff)));
            }
            b"VP8 " => {
                if size < 3 || size > i32::MAX as u32 {
                    return None;
                }
                let start = read::<3>(reader)?;
                if start[0] & 1 != 0 {
                    return Some((0, 0));
                }
                if size < 10 {
                    return None;
                }
                let data = read::<7>(reader)?;
                if &data[..3] != b"\x9d\x01\x2a" {
                    return None;
                }
                return Some((
                    u32::from(le16(&data[3..5]) & 0x3fff),
                    u32::from(le16(&data[5..7]) & 0x3fff),
                ));
            }
            b"ALPH" => return None,
            _ => {
                let skip_bytes = size.checked_add(size & 1)?;
                remaining = remaining.checked_sub(skip_bytes)?;
                skip(reader, skip_bytes as u64)?;
            }
        }
    }
}
