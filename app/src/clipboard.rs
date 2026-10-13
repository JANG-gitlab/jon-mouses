use std::{
    borrow::Cow,
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
};

use arboard::{Clipboard, ImageData};

use crate::wire::Error;

const TEXT: u8 = 0;
const IMAGE: u8 = 1;

pub struct Snapshot {
    pub fingerprint: u64,
    pub encoded: Vec<u8>,
}

enum Content {
    Text(String),
    Image { width: usize, height: usize, rgba: Vec<u8> },
}

impl Content {
    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        match self {
            Content::Text(text) => (TEXT, text).hash(&mut hasher),
            Content::Image { width, height, rgba } => (IMAGE, width, height, rgba).hash(&mut hasher),
        }
        hasher.finish()
    }
}

fn current(clipboard: &mut Clipboard) -> Option<Content> {
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => Some(Content::Text(text)),
        _ => clipboard.get_image().ok().map(|image| Content::Image {
            width: image.width,
            height: image.height,
            rgba: image.bytes.into_owned(),
        }),
    }
}

/// Reads the clipboard and encodes it, unless it still holds what was last
/// shared: images are only compressed when they actually changed.
pub fn read_changed(known: Option<u64>) -> Option<Snapshot> {
    let content = current(&mut Clipboard::new().ok()?)?;
    let fingerprint = content.fingerprint();
    if known == Some(fingerprint) {
        return None;
    }
    let encoded = match content {
        Content::Text(text) => [&[TEXT][..], text.as_bytes()].concat(),
        Content::Image { width, height, rgba } => {
            let mut encoded = vec![IMAGE];
            encoded.extend((width as u32).to_be_bytes());
            encoded.extend((height as u32).to_be_bytes());
            let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
            encoded
        }
    };
    Some(Snapshot { fingerprint, encoded })
}

/// Writes what the peer shared and returns the fingerprint of the clipboard
/// as the system now reports it, so it is not sent straight back.
pub fn write(encoded: &[u8]) -> Result<Option<u64>, Error> {
    let (&kind, body) = encoded.split_first().ok_or("empty clipboard")?;
    let mut clipboard = Clipboard::new()?;
    match kind {
        TEXT => clipboard.set_text(String::from_utf8_lossy(body).into_owned())?,
        IMAGE => {
            let (size, png_bytes) = body.split_at_checked(8).ok_or("truncated image")?;
            let width = u32::from_be_bytes(size[..4].try_into()?) as usize;
            let height = u32::from_be_bytes(size[4..].try_into()?) as usize;
            let mut reader = png::Decoder::new(Cursor::new(png_bytes)).read_info()?;
            let mut rgba = vec![0; reader.output_buffer_size().ok_or("image too large")?];
            let frame = reader.next_frame(&mut rgba)?;
            rgba.truncate(frame.buffer_size());
            if frame.width as usize != width || frame.height as usize != height || frame.color_type != png::ColorType::Rgba {
                return Err("unexpected image format".into());
            }
            clipboard.set_image(ImageData { width, height, bytes: Cow::Owned(rgba) })?;
        }
        _ => return Err("unknown clipboard content".into()),
    }
    Ok(current(&mut clipboard).map(|content| content.fingerprint()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses the real clipboard"]
    fn text_survives_the_round_trip() {
        let Ok(mut clipboard) = Clipboard::new() else {
            return;
        };
        let saved = clipboard.get_text().ok();
        clipboard.set_text("Jon Mouse's ✓ 123").unwrap();
        let snapshot = read_changed(None).unwrap();
        assert!(read_changed(Some(snapshot.fingerprint)).is_none());
        let written = write(&snapshot.encoded).unwrap();
        assert_eq!(written, Some(snapshot.fingerprint));
        if let Some(text) = saved {
            let _ = clipboard.set_text(text);
        }
    }

    #[test]
    #[ignore = "uses the real clipboard"]
    fn image_survives_the_round_trip() {
        let Ok(mut clipboard) = Clipboard::new() else {
            return;
        };
        let saved = clipboard.get_text().ok();
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|value| value as u8 * 10).collect();
        clipboard.set_image(ImageData { width: 3, height: 2, bytes: Cow::Borrowed(&rgba) }).unwrap();
        let snapshot = read_changed(None).unwrap();
        assert_eq!(snapshot.encoded[0], IMAGE);
        write(&snapshot.encoded).unwrap();
        let image = clipboard.get_image().unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        if let Some(text) = saved {
            let _ = clipboard.set_text(text);
        }
    }
}
