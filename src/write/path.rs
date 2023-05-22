use crate::util::Context;
use crate::util::TransformExt;
use crate::write::render::Render;
use pdf_writer::types::{LineCapStyle, LineJoinStyle, ColorSpaceOperand};
use pdf_writer::{Content, PdfWriter};
use usvg::Fill;
use usvg::Stroke;
use usvg::{FillRule, LineCap, LineJoin, Node, Paint, PathSegment, Visibility};
use crate::util::RgbColor;

impl Render for usvg::Path {
    fn render(
        &self,
        node: &Node,
        writer: &mut PdfWriter,
        content: &mut Content,
        ctx: &mut Context,
    ) {
        if self.visibility != Visibility::Visible {
            return;
        }

        content.save_state();
        content.transform(self.transform.get_transform());

        //TODO: Change to SRGB
        content.set_fill_color_space(ColorSpaceOperand::DeviceRgb);
        content.set_stroke_color_space(ColorSpaceOperand::DeviceRgb);

        if let Some(stroke) = &self.stroke {
            set_stroke(stroke, content);
        }

        if let Some(fill) = &self.fill {
            set_fill(fill, content);
        }

        draw_path(self.data.segments(), content);
        finish_path(self.stroke.as_ref(), self.fill.as_ref(), content);

        content.restore_state();
    }
}

fn draw_path(path_data: impl Iterator<Item = PathSegment>, content: &mut Content) {
    for operation in path_data {
        match operation {
            PathSegment::MoveTo { x, y } => content.move_to(x as f32, y as f32),
            PathSegment::LineTo { x, y } => content.line_to(x as f32, y as f32),
            PathSegment::CurveTo { x1, y1, x2, y2, x, y } => content
                .cubic_to(x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32),
            PathSegment::ClosePath => content.close_path(),
        };
    }
}

fn finish_path(stroke: Option<&Stroke>, fill: Option<&Fill>, content: &mut Content) {
    match (stroke, fill.map(|f| f.rule)) {
        (Some(_), Some(FillRule::NonZero)) => content.fill_nonzero_and_stroke(),
        (Some(_), Some(FillRule::EvenOdd)) => content.fill_even_odd_and_stroke(),
        (None, Some(FillRule::NonZero)) => content.fill_nonzero(),
        (None, Some(FillRule::EvenOdd)) => content.fill_even_odd(),
        (Some(_), _) => content.stroke(),
        (None, _) => content.end_path(),
    };
}

fn set_stroke(stroke: &Stroke, content: &mut Content) {
    content.set_line_width(stroke.width.get() as f32);
    content.set_miter_limit(stroke.miterlimit.get() as f32);

    match stroke.linecap {
        LineCap::Butt => content.set_line_cap(LineCapStyle::ButtCap),
        LineCap::Round => content.set_line_cap(LineCapStyle::RoundCap),
        LineCap::Square => {
            content.set_line_cap(LineCapStyle::ProjectingSquareCap)
        }
    };

    match stroke.linejoin {
        LineJoin::Miter => content.set_line_join(LineJoinStyle::MiterJoin),
        LineJoin::Round => content.set_line_join(LineJoinStyle::RoundJoin),
        LineJoin::Bevel => content.set_line_join(LineJoinStyle::BevelJoin),
    };

    if let Some(dasharray) = &stroke.dasharray {
        content.set_dash_pattern(
            dasharray.iter().map(|&x| x as f32),
            stroke.dashoffset,
        );
    }

    match &stroke.paint {
        Paint::Color(c) => {
            content.set_stroke_color(RgbColor::from(*c).to_array());
        }
        _ => todo!(),
    }
}

fn set_fill(fill: &Fill, content: &mut Content) {
    let paint = &fill.paint;

    match paint {
        Paint::Color(c) => {
            content.set_fill_color(RgbColor::from(*c).to_array());
        }
        _ => {}
    }
}