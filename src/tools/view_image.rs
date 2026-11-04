use super::{
    Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, permissions, target,
};
use crate::{agent::message::Attachment, api::models::Model, io, limits, text::template::fill};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "view_image",
        include_str!("../../data/tools/view_image.md"),
        include_str!("../../data/tools/view_image.schema.json"),
    )
});
#[derive(Deserialize)]
struct Args {
    path: String,
}
#[derive(Deserialize)]
struct Text {
    attached: String,
    unsupported: String,
    unavailable: String,
    not_file: String,
    missing: String,
    read_failed: String,
}
static TEXT: LazyLock<Text> =
    LazyLock::new(|| toml::from_str(include_str!("../../data/image.toml")).expect("image text"));

pub fn supported(model: &Model) -> bool {
    model
        .input_modalities
        .as_ref()
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.eq_ignore_ascii_case("image")))
}

pub struct ViewImage;
impl Tool for ViewImage {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args = parse::<Args>(args)?;
        if cx.run.is_some_and(|run| !supported(run.model)) {
            return Err(Refusal::Refused(TEXT.unavailable.clone()));
        }
        let target = target::resolve(&cx.app.paths, &args.path).map_err(Refusal::Refused)?;
        Ok(Check::new(target::asks(permissions::READ.name, &target)))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args = match parse_or_fail::<Args>(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            if cx.run.is_some_and(|run| !supported(run.model)) {
                return Output::error(TEXT.unavailable.clone());
            }
            let target = match target::resolve(&cx.app.paths, &args.path) {
                Ok(target) => target,
                Err(why) => return Output::error(why),
            };
            match target.kind() {
                Ok(io::fs::Kind::File) => {}
                Ok(kind) => {
                    let text = if kind == io::fs::Kind::Missing {
                        &TEXT.missing
                    } else {
                        &TEXT.not_file
                    };
                    return Output::error(fill(text, &[("path", &target.shown)]));
                }
                Err(why) => return Output::error(why),
            }
            let bytes = match io::fs::read_bytes_limited(
                &target.path,
                limits::get().tools.image_max_bytes.value,
            ) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => {
                    return Output::error(fill(&TEXT.missing, &[("path", &target.shown)]));
                }
                Err(error) => {
                    return Output::error(fill(
                        &TEXT.read_failed,
                        &[("path", &target.shown), ("error", &error.to_string())],
                    ));
                }
            };
            let Some(mime) = mime(&bytes) else {
                return Output::error(fill(&TEXT.unsupported, &[("path", &target.shown)]));
            };
            let mut out = Output::ok(fill(
                &TEXT.attached,
                &[
                    ("path", &target.shown),
                    ("mime", mime),
                    ("bytes", &bytes.len().to_string()),
                ],
            ));
            out.details = Some(json!({"bytes":bytes.len(),"mime_type":mime}));
            out.attachments.push(Attachment {
                mime_type: mime.into(),
                data: STANDARD.encode(&bytes),
                filename: Some(target.shown),
            });
            out
        })
    }
}

// File signatures determine the MIME type because extensions can be wrong.
fn mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        Some("image/png")
    } else if bytes.len() >= 4 && bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.len() >= 13 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        Some("image/gif")
    } else if bytes.len() >= 20
        && bytes.starts_with(b"RIFF")
        && &bytes[8..12] == b"WEBP"
        && matches!(&bytes[12..16], b"VP8 " | b"VP8L" | b"VP8X")
    {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";
    #[tokio::test]
    async fn attaches_image_bytes_without_exposing_base64_in_text_and_checks_read_permissions() {
        let p = Project::new();
        io::fs::write_atomic(
            &p.app.paths.project.join("screen.bin"),
            &STANDARD.decode(PNG).unwrap(),
            io::fs::Access::Shared,
        )
        .unwrap();
        let args = json!({"path":"screen.bin"});
        let check = ViewImage.check(&args, &p.cx()).unwrap();
        assert_eq!(check.asks[0].permission, permissions::READ.name);
        let out = ViewImage.run(args, &p.cx()).await;
        assert!(!out.is_error);
        assert!(!out.content.contains(PNG));
        assert_eq!(out.attachments[0].data, PNG);
        assert_eq!(out.attachments[0].mime_type, "image/png");
        let outside = ViewImage
            .check(
                &json!({"path":p.app.paths.project.parent().unwrap().join("outside.png")}),
                &p.cx(),
            )
            .unwrap();
        assert!(
            outside
                .asks
                .iter()
                .any(|ask| ask.permission == permissions::EXTERNAL_DIRECTORY.name)
        );
    }
    #[tokio::test]
    async fn rejects_missing_binary_directory_and_oversized_inputs() {
        let p = Project::new();
        p.write("fake.png", "not an image");
        for path in ["missing.png", "fake.png", "."] {
            assert!(ViewImage.run(json!({"path":path}), &p.cx()).await.is_error);
        }
        let bytes = vec![0; limits::get().tools.image_max_bytes.value + 1];
        io::fs::write_atomic(
            &p.app.paths.project.join("huge.png"),
            &bytes,
            io::fs::Access::Shared,
        )
        .unwrap();
        let out = ViewImage.run(json!({"path":"huge.png"}), &p.cx()).await;
        assert!(out.is_error);
        assert!(out.attachments.is_empty());
    }
    #[test]
    fn registry_only_advertises_images_for_explicit_vision_support() {
        let p = Project::new();
        let registry = super::super::Registry::builtin(&p.app);
        let mut model = crate::api::models::fixture().remove(0);
        for kinds in [
            None,
            Some(vec!["text".into()]),
            Some(vec!["text".into(), "image".into()]),
        ] {
            model.input_modalities = kinds;
            assert_eq!(
                registry
                    .for_model(&model, false)
                    .get("view_image")
                    .is_some(),
                supported(&model)
            );
        }
    }

    #[test]
    fn detects_supported_headers_without_trusting_the_extension() {
        assert_eq!(mime(&STANDARD.decode(PNG).unwrap()), Some("image/png"));
        assert_eq!(mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(
            mime(b"GIF89a\x01\x00\x01\x00\x00\x00\x00"),
            Some("image/gif")
        );
        assert_eq!(
            mime(b"RIFF\x14\x00\x00\x00WEBPVP8L\x00\x00\x00\x00"),
            Some("image/webp")
        );
        for bytes in [b"<svg/>".as_slice(), b"\x89PNG", b"RIFF\0\0\0\0WAVEdata"] {
            assert!(mime(bytes).is_none());
        }
    }
}
