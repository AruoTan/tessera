//! Compile the two monochrome SVGs into vector drawing commands. The parser
//! runs on the build host; the application only needs the existing rasterizer.
use std::{env, fmt::Write, fs, path::PathBuf};
use usvg::{Node, tiny_skia_path::PathSegment};

pub fn compile() {
    let mut code = String::new();
    for name in ["upload", "download"] {
        let path = format!("assets/icons/cloud-{name}.svg");
        println!("cargo:rerun-if-changed={path}");
        let svg = fs::read_to_string(path).expect("cannot read icon SVG");
        let tree = usvg::Tree::from_str(&svg, &Default::default()).expect("invalid icon SVG");
        writeln!(
            code,
            "fn rasterize_{name}(size: u32) -> Option<Vec<u8>> {{\n\
             let mut image = Pixmap::new(size, size)?;\n\
             let scale = Transform::from_scale(size as f32 / {:?}, size as f32 / {:?});",
            tree.size().width(),
            tree.size().height(),
        )
        .unwrap();
        group(tree.root(), &mut code);
        code.push_str("Some(image.pixels().iter().map(|p| p.alpha()).collect())\n}\n");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(output.join("icons.rs"), code).expect("cannot write compiled icons");
}

fn group(group: &usvg::Group, code: &mut String) {
    // Reject effects this small monochrome compiler cannot faithfully preserve.
    assert!(
        group.opacity().get() == 1.0
            && group.blend_mode() == usvg::BlendMode::Normal
            && !group.isolate()
            && group.clip_path().is_none()
            && group.mask().is_none()
            && group.filters().is_empty(),
        "unsupported icon group effects"
    );
    for node in group.children() {
        let path = match node {
            Node::Group(child) => {
                self::group(child, code);
                continue;
            }
            Node::Path(path) => path,
            _ => panic!("icons must contain only vector paths"),
        };
        if !path.is_visible() {
            continue;
        }
        code.push_str("{ let mut path = PathBuilder::new();\n");
        for segment in path.data().segments() {
            match segment {
                PathSegment::MoveTo(p) => writeln!(code, "path.move_to({:?}, {:?});", p.x, p.y),
                PathSegment::LineTo(p) => writeln!(code, "path.line_to({:?}, {:?});", p.x, p.y),
                PathSegment::QuadTo(a, b) => writeln!(
                    code,
                    "path.quad_to({:?}, {:?}, {:?}, {:?});",
                    a.x, a.y, b.x, b.y
                ),
                PathSegment::CubicTo(a, b, c) => writeln!(
                    code,
                    "path.cubic_to({:?}, {:?}, {:?}, {:?}, {:?}, {:?});",
                    a.x, a.y, b.x, b.y, c.x, c.y
                ),
                PathSegment::Close => writeln!(code, "path.close();"),
            }
            .unwrap();
        }
        writeln!(code, "let path = path.finish()?;\nlet transform = scale.pre_concat({:?});\nlet mut paint = Paint::default();", path.abs_transform()).unwrap();
        let fill = |code: &mut String| {
            if let Some(fill) = path.fill() {
                assert!(
                    matches!(fill.paint(), usvg::Paint::Color(_)),
                    "icons need solid fills"
                );
                let rule = match fill.rule() {
                    usvg::FillRule::NonZero => "Winding",
                    usvg::FillRule::EvenOdd => "EvenOdd",
                };
                writeln!(code, "paint.set_color_rgba8(0, 0, 0, {});\nimage.fill_path(&path, &paint, FillRule::{rule}, transform, None);", fill.opacity().to_u8()).unwrap();
            }
        };
        let stroke = |code: &mut String| {
            if let Some(stroke) = path.stroke() {
                assert!(
                    matches!(stroke.paint(), usvg::Paint::Color(_)) && stroke.dasharray().is_none(),
                    "icons need solid, undashed strokes"
                );
                let stroke_style = stroke.to_tiny_skia();
                writeln!(code, "paint.set_color_rgba8(0, 0, 0, {});\nimage.stroke_path(&path, &paint, &Stroke {{ width: {:?}, miter_limit: {:?}, line_cap: LineCap::{:?}, line_join: LineJoin::{:?}, dash: None }}, transform, None);", stroke.opacity().to_u8(), stroke_style.width, stroke_style.miter_limit, stroke_style.line_cap, stroke_style.line_join).unwrap();
            }
        };
        if path.paint_order() == usvg::PaintOrder::FillAndStroke {
            fill(code);
            stroke(code);
        } else {
            stroke(code);
            fill(code);
        }
        code.push_str("}\n");
    }
}
