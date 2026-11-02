//! Images in tool results, as each wire carries them.
//!
//! A tool such as `computer.screenshot` returns text and an image. The three
//! wires differ in where an image may go (Anthropic: inside `tool_result`;
//! Responses: inside `function_call_output`; chat completions: not in a
//! `tool` message at all), but they agree on what the model reads when the
//! bytes are gone, which is what this module holds.
//!
//! An image is gone after it was read back from storage (its bytes are never
//! stored, see [`ImagePart`]) or after the turn's window dropped it to keep
//! the prompt small. The model then reads a note in its place, so it knows a
//! picture was there and takes a new one if it needs to see.

use kura_protocol::ImagePart;

/// The tool's text, with a note for each image no longer present.
pub(crate) fn text_with_omissions(output: &str, images: &[ImagePart]) -> String {
    let omitted = images.iter().filter(|image| !image.is_present()).count();
    if omitted == 0 {
        return output.to_string();
    }
    let mut text = output.to_string();
    for _ in 0..omitted {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(ImagePart::OMITTED);
    }
    text
}

/// The images whose bytes are still here.
pub(crate) fn present(images: &[ImagePart]) -> impl Iterator<Item = &ImagePart> {
    images.iter().filter(|image| image.is_present())
}

/// A `data:` URL, the form the OpenAI wires take an inline image in.
pub(crate) fn data_url(image: &ImagePart) -> String {
    format!("data:{};base64,{}", image.media_type, image.data)
}

#[cfg(test)]
pub(crate) fn image(media_type: &str, data: &str) -> ImagePart {
    ImagePart {
        media_type: media_type.into(),
        data: data.into(),
        artifact: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_image_reads_as_a_note() {
        let images = [image("image/png", "AAAA"), image("image/png", "")];
        assert_eq!(
            text_with_omissions("clicked", &images),
            "clicked\n[image omitted]"
        );
        assert_eq!(text_with_omissions("", &images[1..]), "[image omitted]");
        assert_eq!(text_with_omissions("clicked", &images[..1]), "clicked");
        assert_eq!(present(&images).count(), 1);
        assert_eq!(data_url(&images[0]), "data:image/png;base64,AAAA");
    }
}
