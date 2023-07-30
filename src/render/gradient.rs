use std::rc::Rc;

use pdf_writer::types::{MaskType, ShadingType};
use pdf_writer::{Content, Filter, Finish, PdfWriter, Ref};
use usvg::{
    LinearGradient, NonZeroRect, NormalizedF32, Paint, RadialGradient, StopOffset,
    Transform, Units,
};

use crate::util::context::Context;
use crate::util::helper::{NameExt, RectExt, StopExt, TransformExt};

#[derive(PartialEq, Clone, Debug)]
pub struct Stop<const COUNT: usize> {
    pub color: [f32; COUNT],
    pub offset: f32,
}

/// Turn a group into an shading object (including a soft mask if the gradient contains stop opacities).
/// Returns the name (= the name in the `Resources` dictionary) of the shading object and optionally
/// the name of the soft mask.
pub fn create(
    paint: &Paint,
    parent_bbox: &NonZeroRect,
    writer: &mut PdfWriter,
    ctx: &mut Context,
    accumulated_transform: &Transform,
) -> Option<(Rc<String>, Option<Rc<String>>)> {
    match paint {
        Paint::LinearGradient(l) => Some(create_linear_gradient(
            l.clone(),
            parent_bbox,
            writer,
            ctx,
            accumulated_transform,
        )),
        Paint::RadialGradient(r) => Some(create_radial_gradient(
            r.clone(),
            parent_bbox,
            writer,
            ctx,
            accumulated_transform,
        )),
        _ => None,
    }
}

struct GradientProperties {
    coords: Vec<f32>,
    shading_type: ShadingType,
    stops: Vec<usvg::Stop>,
    transform: Transform,
    units: Units,
}

fn create_linear_gradient(
    gradient: Rc<LinearGradient>,
    parent_bbox: &NonZeroRect,
    writer: &mut PdfWriter,
    ctx: &mut Context,
    accumulated_transform: &Transform,
) -> (Rc<String>, Option<Rc<String>>) {
    let properties = GradientProperties {
        coords: vec![gradient.x1, gradient.y1, gradient.x2, gradient.y2],
        shading_type: ShadingType::Axial,
        stops: gradient.stops.clone(),
        transform: gradient.transform,
        units: gradient.units,
    };
    create_shading_pattern(&properties, parent_bbox, writer, ctx, accumulated_transform)
}

fn create_radial_gradient(
    gradient: Rc<RadialGradient>,
    parent_bbox: &NonZeroRect,
    writer: &mut PdfWriter,
    ctx: &mut Context,
    accumulated_transform: &Transform,
) -> (Rc<String>, Option<Rc<String>>) {
    let properties = GradientProperties {
        coords: vec![
            gradient.fx,
            gradient.fy,
            0.0,
            gradient.cx,
            gradient.cy,
            gradient.r.get(),
        ],
        shading_type: ShadingType::Radial,
        stops: gradient.stops.clone(),
        transform: gradient.transform,
        units: gradient.units,
    };
    create_shading_pattern(&properties, parent_bbox, writer, ctx, accumulated_transform)
}

fn create_shading_pattern(
    properties: &GradientProperties,
    parent_bbox: &NonZeroRect,
    writer: &mut PdfWriter,
    ctx: &mut Context,
    accumulated_transform: &Transform,
) -> (Rc<String>, Option<Rc<String>>) {
    let pattern_ref = ctx.alloc_ref();

    let soft_mask = if properties.stops.iter().any(|stop| stop.opacity.get() < 1.0) {
        Some(get_soft_mask(properties, parent_bbox, writer, ctx))
    } else {
        None
    };

    let matrix = accumulated_transform
        .pre_concat(if properties.units == Units::ObjectBoundingBox {
            Transform::from_bbox(*parent_bbox)
        } else {
            Transform::default()
        })
        .pre_concat(properties.transform);

    let shading_function_ref =
        get_function(&properties.stops, writer, ctx, false);
    let mut shading_pattern = writer.shading_pattern(pattern_ref);
    let mut shading = shading_pattern.shading();
    shading.shading_type(properties.shading_type);
    shading.color_space().srgb();

    shading.function(shading_function_ref);
    shading.coords(properties.coords.iter().copied());
    shading.extend([true, true]);
    shading.finish();

    shading_pattern.matrix(matrix.to_pdf_transform());
    shading_pattern.finish();

    (ctx.deferrer.add_pattern(pattern_ref), soft_mask)
}

fn get_soft_mask(
    properties: &GradientProperties,
    parent_bbox: &NonZeroRect,
    writer: &mut PdfWriter,
    ctx: &mut Context,
) -> Rc<String> {
    ctx.deferrer.push();
    let x_object_id = ctx.alloc_ref();
    let shading_ref = ctx.alloc_ref();
    let shading_name = ctx.deferrer.add_shading(shading_ref);
    let bbox = ctx.get_rect().to_pdf_rect();

    let transform = properties.transform.pre_concat(
        if properties.units == Units::ObjectBoundingBox {
            Transform::from_bbox(*parent_bbox)
        } else {
            Transform::default()
        },
    );

    let shading_function_ref = get_function(&properties.stops, writer, ctx, true);
    let mut shading = writer.shading(shading_ref);
    shading.shading_type(properties.shading_type);
    shading.color_space().d65_gray();

    shading.function(shading_function_ref);
    shading.coords(properties.coords.iter().copied());
    shading.extend([true, true]);
    shading.finish();

    let mut content = Content::new();
    content.transform(transform.to_pdf_transform());
    content.shading(shading_name.to_pdf_name());
    let content_stream = ctx.finish_content(content);

    let mut x_object = writer.form_xobject(x_object_id, &content_stream);
    ctx.deferrer.pop(&mut x_object.resources());

    x_object
        .group()
        .transparency()
        .isolated(false)
        .knockout(false)
        .color_space()
        .d65_gray();

    if ctx.options.compress {
        x_object.filter(Filter::FlateDecode);
    }

    x_object.bbox(bbox);
    x_object.finish();

    let gs_ref = ctx.alloc_ref();
    let mut gs = writer.ext_graphics(gs_ref);
    gs.soft_mask()
        .subtype(MaskType::Luminosity)
        .group(x_object_id)
        .finish();

    ctx.deferrer.add_graphics_state(gs_ref)
}

fn get_function(
    stops: &[usvg::Stop],
    writer: &mut PdfWriter,
    ctx: &mut Context,
    use_opacities: bool
) -> Ref {
    // Gradients with no stops and only one stop should automatically be converted by resvg
    // into no fill / plain fill, so there should be at least two stops
    debug_assert!(stops.len() > 1);

    let mut stops = stops.to_owned();

    // We manually pad the stops if necessary so that they are always in the range from 0-1
    if let Some(first) = stops.first() {
        if first.offset != NormalizedF32::ZERO {
            let mut new_stop = *first;
            new_stop.offset = StopOffset::new(0.0).unwrap();
            stops.insert(0, new_stop);
        }
    }

    if let Some(last) = stops.last() {
        if last.offset != NormalizedF32::ONE {
            let mut new_stop = *last;
            new_stop.offset = StopOffset::new(1.0).unwrap();
            stops.push(new_stop);
        }
    }


    if use_opacities {
        let stops = stops.iter().map(|s| s.opacity_stops()).collect::<Vec<Stop<1>>>();
        function(&stops, writer, ctx)
    }   else {
        let stops = stops.iter().map(|s| s.color_stops()).collect::<Vec<Stop<3>>>();
        function(&stops, writer, ctx)
    }
}

fn function<const COUNT: usize>(
    stops: &[Stop<COUNT>],
    writer: &mut PdfWriter,
    ctx: &mut Context,
) -> Ref {
    if stops.len() == 2 {
        exponential_function(&stops[0], &stops[1], writer, ctx)
    }   else {
        stitching_function(stops, writer, ctx)
    }
}

/// Create a stitching function for multiple gradient stops.
fn stitching_function<const COUNT: usize>(
    stops: &[Stop<COUNT>],
    writer: &mut PdfWriter,
    ctx: &mut Context,
) -> Ref {
    assert!(!stops.is_empty());

    let reference = ctx.alloc_ref();

    let mut functions = vec![];
    let mut bounds = vec![];
    let mut encode = vec![];

    for window in stops.windows(2) {
        let (first, second) = (&window[0], &window[1]);
        bounds.push(second.offset);
        functions.push(exponential_function(first, second, writer, ctx));
        encode.extend([0.0, 1.0]);
    }

    bounds.pop();

    let mut stitching_function = writer.stitching_function(reference);
    stitching_function.domain([0.0, 1.0]);
    stitching_function.range(get_function_range(COUNT));
    stitching_function.functions(functions);
    stitching_function.bounds(bounds);
    stitching_function.encode(encode);
    reference
}


/// Create an exponential function for two gradient stops.
fn exponential_function<const COUNT: usize>(
    first_stop: &Stop<COUNT>,
    second_stop: &Stop<COUNT>,
    writer: &mut PdfWriter,
    ctx: &mut Context,
) -> Ref {
    let reference = ctx.alloc_ref();
    let mut exp = writer.exponential_function(reference);

    exp.range(get_function_range(COUNT));
    exp.c0(first_stop.color);
    exp.c1(second_stop.color);
    exp.domain([0.0, 1.0]);
    exp.n(1.0);
    exp.finish();
    reference
}

fn get_function_range(count: usize) -> Vec<f32> {
    [0.0, 1.0].repeat(count)
}

