//! Actual drawing code for the plots.
//! It looks like this can be moved to a separate crate?

use std::{
    ops::Range,
    sync::{Arc, LazyLock},
};

use color_eyre::eyre::{Report, eyre};
use crabbyconsole_clap::plot::MetricsKind;
use crabbyconsole_misc::profile;
use godot::builtin::math::ApproxEq;
use num_traits::float::FloatCore; // or `ordered_float::FloatCore?`
use ordered_float::{FloatIsNan, NotNan};
use plotters::{
    backend::BitMapBackend,
    chart::ChartBuilder,
    coord::Shift,
    drawing::{DrawingArea, IntoDrawingArea as _},
    element::Rectangle,
    series::LineSeries,
    style::{
        Color as _, FontStyle, IntoFont as _, RED, ShapeStyle, WHITE, YELLOW,
        text_anchor::{HPos, Pos, VPos},
    },
};

type NN<T> = NotNan<T>;
type NNF32 = NN<f32>;

// Need a macro here, since include_bytes! requires a string literal
macro_rules! asset_path {
    ($rel:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/", $rel)
    };
}

// These need to be macros, or we can't use it in include_bytes!
macro_rules! font_metrics_asset_path {
    () => {
        asset_path!("fonts/plotters/om_tall_plain.ttf")
    };
}

macro_rules! font_sans_serif_asset_path {
    () => {
        asset_path!("fonts/plotters/NFPixels-Regular.ttf")
    };
}

#[derive(Clone)]
struct PlottersFontLayout {
    font_metrics_name: &'static str,
    font_sans_serif_name: &'static str,

    font_metrics_size: u32,
    font_sans_serif_size: u32,

    font_metrics_line_height: u32,
    font_metrics_margin: i32,
    font_sans_serif_y_label_area_size: i32,
}

static FONT_LAYOUT: LazyLock<Arc<Result<PlottersFontLayout, Report>>> = LazyLock::new(|| {
    let font_metrics_name = "plotters-metrics";
    let font_sans_serif_name = "sans-serif"; // "sans-serif" is the default font plotters uses

    let a = (|| {
        // The fonts are only 10-30kb so should be fine to embed this.
        let font_metrics_bytes = include_bytes!(font_metrics_asset_path!());
        let font_sans_serif_bytes = include_bytes!(font_sans_serif_asset_path!());

        // Call this to load and register the font if it wasn't registered yet.
        // We need this, since we use plotters with the `ab_glyph` feature, which enables the pure-Rust font renderer.
        // This renderer is way faster than the `ttf` one, since it doesn't try to load the font from disk every time we render anything.
        // However, `ab_glyph` has no default font, so if you don't register a font, rendering will panic.

        plotters::style::register_font(font_metrics_name, FontStyle::Normal, font_metrics_bytes)
            .map_err(|_e| eyre!("failed to load font {}", font_metrics_asset_path!()))?;

        plotters::style::register_font(
            font_sans_serif_name,
            FontStyle::Normal,
            font_sans_serif_bytes,
        )
        .map_err(|_e| eyre!("failed to load font {}", font_sans_serif_asset_path!()))?;

        Ok(PlottersFontLayout {
            font_metrics_name,
            font_sans_serif_name,
            // ------
            font_metrics_size: 16,
            font_sans_serif_size: 15,
            // 15 for NFPixels
            // 16 for om_tall_plain
            // ------
            font_metrics_line_height: 16,
            // 10 for NFpixels
            // 16 for om_tall_plain
            // ------
            font_metrics_margin: 8,
            // 4 for NFpixels
            // 8 for om_tall_plain
            // ------
            font_sans_serif_y_label_area_size: 45, // <-- should be fairly big to fit large numbers like vram usage
        })
    })();
    Arc::new(a)
});

/// Loads font from disk the first time you call it, then afterwards loads the cached version.
/// If font loading fails, returns Err.
/// Returns information about the font layout.
fn load_cached_font() -> Result<PlottersFontLayout, Report> {
    let font_data: &PlottersFontLayout =
        FONT_LAYOUT.as_ref().as_ref().map_err(|e| eyre!("{e:?}"))?;

    Ok(font_data.clone()) // should be fine to clone it
}

pub(super) fn draw_line_plot(
    buffer: &mut [u8],
    data: Vec<(f32, f32)>, // cannot be  &[(f32, f32)] or plotters will give a vague error
    (width, height): (u32, u32),
    x_range: Option<Range<NNF32>>,
    y_range: Option<Range<NNF32>>,
    thresholds: Vec<NNF32>,
    metrics_kind: MetricsKind,
) -> Result<(), Report> {
    let font = load_cached_font()?;

    // According to profiler it spends like 85% of the time drawing text.
    // So keep text to a minimum.
    // Update: now using ab_glyph, much faster!
    {
        let (min_x, max_x, min_y, max_y) = finalize_bounds(&data, x_range, y_range)?;
        let root = BitMapBackend::with_buffer(buffer, (width, height)).into_drawing_area();
        //  root.fill(&WHITE.mix(0.1))?; // <= slow

        let mut chart = ChartBuilder::on(&root)
            .margin(6)
            .x_label_area_size(15)
            .y_label_area_size(font.font_sans_serif_y_label_area_size)
            .build_cartesian_2d((*min_x)..(*max_x), (*min_y)..(*max_y))?;

        // Draw x/y axes
        // HACK - skip drawing line if any value is nan/inf, otherwise plotters loops infinitely
        let all_finite = data.iter().all(|(x, y)| x.is_finite() && y.is_finite());
        if all_finite {
            chart
                .configure_mesh()
                .axis_style(WHITE.mix(0.5)) // axis number text
                // TODO maybe we can use the NFpixels font for labels? since they don't change much?
                .label_style(
                    (font.font_sans_serif_name, font.font_sans_serif_size)
                        .into_font()
                        .color(&WHITE.mix(0.5)),
                )
                .light_line_style(WHITE.mix(0.1))
                .bold_line_style(WHITE.mix(0.2))
                .x_labels(5) // grid lines/ticks on the x-axis
                .x_max_light_lines(2) // minor lines
                .y_labels(5) // grid lines/ticks on the y-axis
                .y_max_light_lines(2) // minor lines
                .draw()?; // <-- this call loops infinitely if you feed nan into :plot line
        } else {
            tracing::warn!("data contains inf/NaN, not drawing it to prevent infinite loop");
        }

        // Draw thresholds
        for threshold in thresholds {
            // we can actually draw a dashed line using DashedLineSeries, may be slow though, also needs the `line_series` feature
            chart.draw_series(LineSeries::new(
                vec![(*min_x, *threshold), (*max_x, *threshold)],
                ShapeStyle::from(&RED.mix(0.6)).stroke_width(1),
            ))?;
        }

        // TODO draw metrics after drawing the line! otherwise the line may occlude it
        // But LineSeries moves data so we can't use it anymore...
        if metrics_kind != MetricsKind::None {
            draw_metrics(metrics_kind, &data, &root, &font, width)?;
        }

        chart.draw_series(LineSeries::new(
            data,
            ShapeStyle::from(&YELLOW).stroke_width(1), //width 2 = kinda ugly
        ))?;

        root.present()?;
    }
    Ok(())
}

pub(super) fn draw_histo_plot(
    buffer: &mut [u8],
    data: Vec<(f32, f32)>, // cannot be  &[(f32, f32)] or plotters will give a vague error
    (width, height): (u32, u32),
    x_range: Option<Range<NNF32>>,
    y_range: Option<Range<NNF32>>,
    thresholds: Vec<NNF32>,
    metrics_kind: MetricsKind,
) -> Result<(), Report> {
    let font = load_cached_font()?;

    // Logarithmic scale would be nice here, but sadly not supported
    // https://github.com/plotters-rs/plotters/issues/708

    {
        let root = BitMapBackend::with_buffer(buffer, (width, height)).into_drawing_area();
        //  root.fill(&WHITE.mix(0.1))?; // <= slow

        let num_bins = 100; // TODO make this configurable?
        let binned = profile!(bin_data(&data, num_bins)); // seems fast enough
        if binned.is_empty() {
            // TODO this is inconsistent with draw_line_plot
            // but needed otherwise max_x panics
            return Err(eyre!("no data to plot"));
        }

        //  let bin_width = binned[0].1 - binned[0].0;

        let x_range = x_range
            .or_else(|| {
                // If x_range wasn't passed by the user, calculate it from the bins
                // range = start of first bin ... end of last bin
                let (min_x, max_x) = { (binned[0].0, binned.last().unwrap().1) };
                let min_x = NN::try_from(min_x).ok()?;
                let max_x = NN::try_from(max_x).ok()?;
                Some(min_x..max_x)
            })
            .ok_or_else(|| {
                eyre!("x_range is NaN, did you feed a Inf or NaN into the histogram?")
            })?;

        let y_range = y_range
            .or_else(|| {
                // If y_range wasn't passed by the user, calculate it from the maximum bin value
                let max_bin_val = binned.iter().map(|(_, _, sum)| *sum).fold(0f32, f32::max);

                let min_y = nn(0.0);
                let max_y = NN::try_from((max_bin_val * 1.1).max(1.0)).ok()?; // add little y margin
                Some(min_y..max_y)
            })
            .ok_or_else(|| {
                eyre!("y_range is NaN, did you feed a Inf or NaN into the histogram?")
            })?;

        let mut chart = ChartBuilder::on(&root)
            .margin(6)
            .x_label_area_size(15)
            .y_label_area_size(font.font_sans_serif_y_label_area_size) // vvv TODO add x margin in case min_x == max_x?
            .build_cartesian_2d(x_range.to_float_range(), y_range.clone().to_float_range())?;

        let all_finite = data.iter().all(|(x, y)| x.is_finite() && y.is_finite());
        if all_finite {
            chart
                .configure_mesh()
                .axis_style(WHITE.mix(0.5)) // axis number text
                .label_style(
                    (font.font_sans_serif_name, font.font_sans_serif_size)
                        .into_font()
                        .color(&WHITE.mix(0.5)),
                )
                .light_line_style(WHITE.mix(0.1))
                .bold_line_style(WHITE.mix(0.2))
                .x_labels(5) // grid lines/ticks on the x-axis
                .x_max_light_lines(2) // minor lines
                .y_labels(5) // grid lines/ticks on the y-axis
                .y_max_light_lines(2) // minor lines
                .draw()?; //<-- TODO freezes here
        } else {
            tracing::warn!("data contains inf/NaN, not drawing it to prevent infinite loop");
        }

        // Now just drawing manually since we need non-discrete x-axis.
        // See https://users.rust-lang.org/t/histogram-of-floating-point-data-with-plotters-rs-blank-graph/
        // And https://github.com/plotters-rs/plotters/issues/42#issuecomment-538181433

        // Draw thresholds
        for threshold in thresholds {
            chart.draw_series(LineSeries::new(
                vec![(*threshold, *y_range.start), (*threshold, *y_range.end)],
                ShapeStyle::from(&RED.mix(0.5)).stroke_width(2), // width=1 is hard to see
            ))?;
        }

        // TODO draw metrics after histo or it may get occluded
        if metrics_kind != MetricsKind::None {
            draw_metrics(metrics_kind, &data, &root, &font, width)?;
        }

        chart.draw_series(binned.into_iter().map(|(bin_min, bin_max, sum)| {
            Rectangle::new([(bin_min, 0.), (bin_max, sum)], YELLOW.mix(0.5).filled())
        }))?;

        root.present()?;
    }
    Ok(())
}

fn draw_metrics(
    metrics_kind: MetricsKind,
    data: &[(f32, f32)],
    root: &DrawingArea<BitMapBackend<'_>, Shift>,
    font: &PlottersFontLayout,
    width: u32,
) -> Result<(), Report> {
    let metrics = profile!(Metrics::new(data)); // seems fast enough (<0.037ms for n=300)

    if let Some(Metrics {
        min,
        max,
        avg,
        last,
        p95,
        std_dev,
        cv,
    }) = metrics
    {
        let mut metrics_texts = vec![
            format!(
                "min: {} max: {} range: {}",
                format_dynamic(min),
                format_dynamic(max),
                format_dynamic(max - min),
            ),
            format!(
                "last: {} avg: {} n: {}",
                format_dynamic(last),
                format_dynamic(avg),
                data.len(),
            ),
        ];

        // Note: smallvec may be faster here to avoid reallocation of the metrics_texts vec
        if metrics_kind == MetricsKind::Advanced {
            metrics_texts.push(format!(
                "p95: {} stdev: {} cv: {}%",
                format_dynamic(p95),
                format_dynamic(std_dev),
                if let Some(cv) = cv {
                    format_dynamic(cv * 100.) // cv <5% = buttery smooth
                } else {
                    "?".into()
                },
            ));
        }

        for (i, metrics_text) in metrics_texts.iter().enumerate() {
            let x = width as i32 - font.font_metrics_margin;
            let y = font.font_metrics_margin + font.font_metrics_line_height as i32 * (i as i32);
            let style = &(font.font_metrics_name, font.font_metrics_size)
                .into_font()
                .color(&WHITE.mix(0.9))
                .pos(Pos::new(HPos::Right, VPos::Top)); // anchor to top-right corner
            root.draw_text(metrics_text, style, (x, y))?;
        }
    }

    Ok(())
}

// TOO maybe use hdrhistogram crate here?
fn bin_data(data: &[(f32, f32)], num_bins: usize) -> Vec<(f32, f32, f32)> {
    if data.is_empty() || num_bins == 0 {
        return Vec::new();
    }

    let min_y = data.iter().map(|(_, y)| *y).fold(f32::INFINITY, f32::min);
    let max_y = data
        .iter()
        .map(|(_, y)| *y)
        .fold(f32::NEG_INFINITY, f32::max);

    let range = (max_y - min_y).max(f32::EPSILON);
    let bin_width = range / num_bins as f32;

    let mut bins = vec![0f32; num_bins];

    for (_, y) in data {
        let mut idx = ((y - min_y) / bin_width) as usize;
        if idx >= num_bins {
            idx = num_bins - 1;
        }
        bins[idx] += 1.0;
    }

    bins.iter()
        .enumerate()
        .map(|(i, &count)| {
            let bin_start = min_y + i as f32 * bin_width;
            let bin_end = bin_start + bin_width;
            (bin_start, bin_end, count)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Metrics {
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) avg: f32,
    pub(super) last: f32,
    pub(super) p95: f32,
    pub(super) std_dev: f32,
    pub(super) cv: Option<f32>, // coefficient of variation (NOT covariance)
}

impl Metrics {
    pub(super) fn new(data: &[(f32, f32)]) -> Option<Self> {
        let last = data.last()?.1; // returns None if data is empty

        let mut ys = data.iter().map(|&(_x, y)| y).collect::<Vec<_>>();
        let n = data.len();
        let avg = ys.iter().sum::<f32>() / n as f32;

        let variance = data
            .iter()
            .map(|&(_x, y)| {
                let d = y - avg;
                d * d
            })
            .sum::<f32>()
            / n as f32; // or n-1 depending on your interpretation of standard deviation
        let std_dev = variance.sqrt();

        // calculate 95th percentile by sorting ys
        ys.sort_unstable_by(|a, b| a.total_cmp(b)); // <-- unstable sort = faster

        let idx = ((0.95 * n as f32).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1);

        let p95 = ys[idx];

        let min = *ys.first().unwrap(); // safe unwrap
        let max = *ys.last().unwrap(); // safe unwrap

        let cv = if avg.approx_eq(&0.0) {
            None // avoid division by 0
        } else {
            Some(std_dev / avg)
        };

        Some(Self {
            min,
            max,
            avg,
            last,
            p95,
            std_dev,
            cv,
        })
    }
}

/// Format a float dynamically, based on its size.
/// So larger numbers get fewer decimals, while smaller numbers get more decimals.
/// TODO make this generic over f32/f64
fn format_dynamic(x: f32) -> String {
    fn dynamic_precision(x: f32, min_decimals: usize, max_decimals: usize) -> usize {
        if x == 0. || !x.is_finite() {
            return min_decimals;
        }
        // magnitude = number of digits before the decimal point (zero if x < 1)
        let magnitude = x.abs().log10().floor() as i32;

        let sigfigs = 3; // aim for about 3 significant figures total
        let precision = sigfigs - 1 - magnitude;

        precision.max(min_decimals as i32).min(max_decimals as i32) as usize
    }

    let precision = dynamic_precision(x, 0, 5);
    format!("{:.*}", precision, x)
}

fn compute_bounds(data: &[(f32, f32)]) -> Option<(f32, f32, f32, f32)> {
    if data.is_empty() {
        return None;
    }

    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for &(x, y) in data {
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }

    let result = Some((min_x, max_x, min_y, max_y));
    //dbg!(result);
    result
}

fn finalize_bounds(
    data: &[(f32, f32)],
    x_range: Option<Range<NNF32>>,
    y_range: Option<Range<NNF32>>,
) -> Result<(NNF32, NNF32, NNF32, NNF32), Report> {
    let epsilon = NN::try_from(f32::EPSILON).unwrap();

    let (auto_min_x, auto_max_x, auto_min_y, auto_max_y) = compute_bounds(data).unwrap_or_default();

    let x_range = x_range
        .or_else(|| (auto_min_x..auto_max_x).to_not_nan_range().ok())
        .unwrap_or_else(|| nn(-1.)..nn(0.)); // Use default range if NaN

    let y_range = y_range
        .or_else(|| (auto_min_y..auto_max_y).to_not_nan_range().ok())
        .unwrap_or_else(|| nn(-1.)..nn(1.)); // Use default range if NaN

    let margin_frac = nn(0.05); // Fraction of the range to pad on each side (0.05 = 5%)

    // TODO maybe we should allow NaN and just use dummy values in that case?
    let x_diff = x_range
        .end
        .safe_sub(x_range.start)
        .unwrap_or_else(|| nn(1.0)) // Use default range if NaN
        .max(epsilon);
    let y_diff = y_range
        .end
        .safe_sub(y_range.start)
        .unwrap_or_else(|| nn(2.0)) // Use default range if NaN
        .max(epsilon); // we max here otherwise y_pad = 0 if start == end

    let x_pad = x_diff * margin_frac;
    let y_pad = y_diff * margin_frac;

    Ok((
        NN::try_from(x_range.start - x_pad)?, // TODO use safe_sub here?
        NN::try_from(x_range.end + x_pad)?,
        NN::try_from(y_range.start - y_pad)?,
        NN::try_from(y_range.end + y_pad)?,
    ))
}

// Little helper trait so we can convert Range<NotNan<f32>>  -> Range<f32>
trait ToFloatRange<T> {
    fn to_float_range(self) -> Range<T>;
}

impl<T> ToFloatRange<T> for Range<NotNan<T>>
where
    T: Copy,
{
    fn to_float_range(self) -> Range<T> {
        self.start.into_inner()..self.end.into_inner()
    }
}

// Little helper trait so we can convert  Range<f32> -> Range<NotNan<f32>>

trait ToNotNanRange<T> {
    fn to_not_nan_range(self) -> Result<Range<NotNan<T>>, FloatIsNan>;
}

impl<T: FloatCore> ToNotNanRange<T> for Range<T> {
    fn to_not_nan_range(self) -> Result<Range<NotNan<T>>, FloatIsNan> {
        Ok(NotNan::new(self.start)?..NotNan::new(self.end)?)
    }
}

/// Create a non-Nan from a float. Use this only with literals, don't feed variables/expressions into it.
/// Since it will panic at runtime if it turns out to be NaN anyway.
pub const fn nn(x: f32) -> NotNan<f32> {
    assert!(!x.is_nan()); // compile error if x is NaN
    unsafe { NotNan::new_unchecked(x) }
}

/// Subtracting two `NotNan<f32>`s like `a - b` is not safe; if they are both `inf`, it results in `NaN`.
/// Since the return type is also `NotNan<f32>`, it panics, since it cannot store a `NaN`.
/// This trait aims to prevent that.
trait SafeSub {
    fn safe_sub(self, other: Self) -> Option<Self>
    where
        Self: Sized;
}

impl SafeSub for NotNan<f32> {
    fn safe_sub(self, other: Self) -> Option<Self> {
        // Convert NotNan<f32> -> f32, then subtract, then check if it's NaN
        if (self.into_inner() - other.into_inner()).is_nan() {
            return None;
        }

        Some(self - other)
    }
}
