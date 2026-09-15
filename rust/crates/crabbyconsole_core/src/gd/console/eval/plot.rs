//! This module contains 1. the clap plot stuff and 2. the plot data + texture caching stuff.
//! Plot drawing code was moved to plot_draw.rs

use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use color_eyre::Report;
use crabbyconsole_clap::plot::PlotAction;
use crabbyconsole_misc::{
    FutureTracyExt as _,
    flags::PLOT_USE_TEXTURE_CACHE_FLAG,
    gd::async_node::AsyncGd,
    profile,
    util::{AValue, append},
};
use godot::{
    classes::{Image, ImageTexture, image::Format},
    prelude::*,
};
use indexmap::indexmap;
use mini_moka::unsync::Cache;
use ordered_float::NotNan;
use tap::TapFallible;

use crate::gd::console::{
    CrabConsole,
    eval::{
        ClapSubAction,
        plot_draw::{draw_histo_plot, draw_line_plot, nn},
    },
    job::JobExpressionInner,
    util::BoxedCache,
};

impl ClapSubAction for PlotAction {
    async fn handle(self, console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        let (w, h) = (340, 180); // if h > 200 it will be scaled down by JobResultData::add_to

        match self {
            ref pa @ PlotAction::Line {
                ref time,
                ref y_range,
                ref threshold,
                ref metrics,
                ref rate,
                ref expression,
            } => {
                // TODO maybe trim the start of the expression too? To increase cache hit rate
                let expression: Arc<str> = Arc::from(expression.join(" "));
                let pa = pa.clone();

                // Evaluate the expression
                let value = console
                    .clone() // fast clone
                    .eval_job_without_channel(JobExpressionInner::String(Arc::clone(&expression)))
                    .with_tracy_non_continuous_frame("eval_job_plot")
                    .await?;

                // Try to convert its result to f32
                let value_numeric = value.try_to_relaxed::<f32>().map_err(|e| e.into_erased())?;

                let data = LINE_PLOT_DATA.with_borrow_mut(|cache| {
                    // TODO maybe pass Arc<str> here and store the Arc in the data cache as key
                    gather_data(expression.to_string(), value_numeric, *time, cache)
                });

                let x_range = Some(-time..nn(0.0));

                let plot =
                    get_or_create_rate_limited_plot(pa, w, h, *rate, console, |source_rgb| {
                        profile!(draw_line_plot(
                            source_rgb,
                            // TODO `source_rgb` can't reuse slot.borrow_mut().0.buffer_mut() here
                            // it assumes the thing is RGBA8, not RGB8.
                            // so we do need to reallocate this buffer every time, oh well
                            // maybe make a second buffer inside of it? `buffer_rgb8` or sth?
                            // and then add a method buffer_rgb8_mut()? and pass it to draw_line_plot?
                            data,
                            (w, h),
                            x_range,
                            y_range.clone(),
                            threshold.clone(),
                            *metrics,
                        ))
                    })?;
                Ok(Variant::from(plot))
            }
            ref pa @ PlotAction::Histo {
                ref x_range,
                ref y_range,
                ref threshold,
                ref metrics,
                ref rate,
                ref expression,
            } => {
                let expression: Arc<str> = Arc::from(expression.join(" "));
                let pa = pa.clone();

                let time = NotNan::from(5_u16); // TODO pass `time` in via PlotAction::Histo?

                // Evaluate the expression
                let value = console
                    .clone() // fast clone
                    .eval_job_without_channel(JobExpressionInner::String(Arc::clone(&expression)))
                    .with_tracy_non_continuous_frame("eval_job_plot")
                    .await?;

                // Try to convert its result to f32
                let value_numeric = value.try_to_relaxed::<f32>().map_err(|e| e.into_erased())?;

                let data = HISTO_PLOT_DATA.with_borrow_mut(|cache| {
                    // TODO maybe pass Arc<str> here and store the Arc in the data cache as key
                    gather_data(expression.to_string(), value_numeric, time, cache)
                });

                let plot =
                    get_or_create_rate_limited_plot(pa, w, h, *rate, console, |source_rgb| {
                        profile!(draw_histo_plot(
                            source_rgb,
                            data,
                            (w, h),
                            x_range.clone(), // These clones are kinda wasteful, since `pa` already contains them. Maybe pass them into the closure?
                            y_range.clone(), // So the arg list becomes |source_rgb, x_range, y_range, threshold|?
                            threshold.clone(),
                            *metrics,
                        ))
                    })?;
                Ok(Variant::from(plot))
            }
        }
    }
}

fn get_or_create_rate_limited_plot(
    pa: PlotAction,
    w: u32,
    h: u32,
    rate: NotNan<f32>,
    console: AsyncGd<CrabConsole>,
    draw_plot: impl FnOnce(&mut [u8]) -> Result<(), Report>,
) -> Result<Gd<ImageTexture>, Report> {
    // At this point, we need to call get_or_create_texture BEFORE calling draw_line_plot.
    // That way, we can determine whether or not we're currently rate limited.
    // If not, we just do the current logic. If we are rate limited, however, we DO NOT CALL draw_line_plot AT ALL.
    // Instead, we just return whatever the cache returned (however, if the cache is empty, we need to call draw_line_plot anyway).
    // To make this less complex we return some kind of Guard struct in get_or_create_texture().
    // It returns whether or not we were rate limited, so you can do sth like this:
    // let guard = get_or_create_texture(). if guard.rate_limited() { return guard.texture } else {
    //      let plot = draw_line_plot(guard.buffer)
    //      guard.upload()
    // }
    // so guard.upload() actually takes the vec<u8> buffer and puts it in the Texture2D on the gpu.
    // Beware: I think guard needs to hold a &reference to the StreamingTexture, so we may get lifetime problems.
    // May need to sprinkle a Rc<> in there to fix that.
    //
    // essentially the idea is that get_or_create_texture is split up into two halves A (lookup) and B (upload).
    // then we need to call them in this order: A -> draw_line_plot -> B
    // because B can't happen before draw_line_plot, since we can't upload the texture if we don't have it yet.
    // but A can't happen after draw_line_plot either, because then the rate-limit-check happens after we've already generated the plot,
    // defeating the entire point of the rate limiter.
    // Returning a guard struct is prob the best way to fix this.

    let now = Instant::now();
    let slot = profile!(
        "get_or_create_texture",
        get_or_create_texture(w, h, pa, now, rate, console)
    );

    let rate_limited = now < slot.borrow().1 + Duration::from_secs_f32(1.0 / *slot.borrow().2); // `rate` should not be zero methinks so probably safe
    if rate_limited {
        // generating plots too fast, so re-use the cached one.
        // we do write down the data point though, so we don't lose it
        return Ok(Gd::clone(&slot.borrow().0.texture));
    } else {
        let mut source_rgb = profile!(vec![0u8; (w * h * 3) as usize]); // <- 0.02-0.04ms: TODO can we cache this? as part of the below scheme? i think we can!

        draw_plot(&mut source_rgb)?;

        // We now have the plot's texture data in `source_rgb`: upload it to the gpu
        {
            let mut slot = slot.borrow_mut();
            profile!(slot.0.update_and_upload_rgb8(&source_rgb)); // ~0.2-0.3ms
            slot.1 = Instant::now(); // write down last update time // -> do not use `now`, it will be slightly off here, since draw_line_plot takes some time to generate + upload
            slot.2 = rate; // update the target frequency
        }
    }

    Ok(Gd::clone(&slot.borrow().0.texture))
}

/// TODO this method is showing up in the profiler now, 93% is spent in `collect()`, which spends 50% in malloc!
/// Go re-use that buffer!
fn gather_data(
    expression: String,
    value_numeric: f32,
    time: NotNan<f32>,
    data_cache: &mut Cache<String, PlotCacheValue>,
) -> Vec<(f32, f32)> {
    let now = Instant::now(); // maybe use DateTime now instead?

    // Insert it in the cache, or create a new entry if missing
    let vec = {
        let vec = data_cache.get(&expression);
        let val = (now, value_numeric);

        if let Some(vec) = vec {
            vec.borrow_mut().push_back(val);
            Rc::clone(vec)
        } else {
            let vec = &Default::default();
            data_cache.insert(expression, Rc::clone(vec));
            vec.borrow_mut().push_back(val);
            Rc::clone(vec)
        }
    };

    // Truncate it if it's too long
    {
        let mut vec = vec.borrow_mut();

        // Truncate all entries >x seconds ago
        while vec
            .pop_front_if(|(x, _)| (now - *x).as_secs_f32() > *time)
            .is_some()
        {
            // spin loop
        }

        // Then truncate again if we still have too many data points
        let max_len = (time * 10. * 60.) as usize; // 10x space for 5 seconds of data gathered at 60 fps.
        while vec.len() > max_len {
            vec.pop_front();
        }
    }
    vec.borrow()
        .iter()
        .map(|(x, y)| (-(now - *x).as_secs_f32(), *y)) // <-- do not use elapsed() or it will drift per-entry
        .collect() // <-- TODO slow - go reuse that buffer somehow - or avoid mapping it somehow
}

type PlotCacheValue = Rc<RefCell<VecDeque<(Instant, f32)>>>;
type PlotCache = BoxedCache<String, PlotCacheValue>; // TODO use Arc<str> instead of String so we get string interning

// This is a thread-local, non-thread-safe cache, since it is only used in the main thread.
// No need to pay for expensive cross-thread synchronization then.
thread_local! {

    /// Note this is a global cache, so it is NOT cleared when you reload the console.
    /// Need a `RefCell` here because there is no `get_mut()` or equivalent.
    ///
    /// Warning! `Cache` is a big struct, like 300 bytes just for an empty cache.
    /// If you store it raw in a thread_local!, it will blow up because it doesn't fit in the TLS buffer.
    /// Solution: Box it!
    static LINE_PLOT_DATA: PlotCache = {
         RefCell::new(Box::new(
            Cache::builder()
                .time_to_idle(Duration::from_mins(10)) // Use TTI instead of TTL, otherwise it will expire while you're still using it
                .build()
        ))
    };

    // copy pasted from LINE_PLOT_CACHE
    static HISTO_PLOT_DATA: PlotCache =  {
         RefCell::new(Box::new(
            Cache::builder()
                .time_to_idle(Duration::from_mins(10)) // Use TTI instead of TTL, otherwise it will expire while you're still using it
                .build()
        ))
    }

}

pub struct StreamingTexture {
    pub(super) image: Gd<Image>,
    pub(super) texture: Gd<ImageTexture>, // Maybe store InstanceId here? (that acts as a WeakRef, but godot-rust discourages using it), and get rid of the auto-expiry entirely?
    pub(super) buffer: PackedByteArray,   /* rgba8 */
    // Actually that may not work - maybe add :plot clear instead of auto-expiry?
    // Because auto-expiry is risky and can disconnect texture connections if you don't update the plot in a while
    pub(super) width: u32,
    pub(super) height: u32,
    // TODO add buffer_rgb8? so we can re-use the buffer? may not be worth it
}

impl StreamingTexture {
    /// Paradoxically, creating a rgb8 buffer, converting it to rgba8,
    /// and then uploading it to Godot is actually faster than uploading the rgb8 buffer.
    /// Why? Because godot internally seems to store textures a rgba8,
    /// so that incurs a costly conversion step, creating a fresh new Image every time you update it.
    pub fn new(width: u32, height: u32) -> Self {
        let expected_len_rgba8 = (width * height * 4) as usize;

        let mut buffer = PackedByteArray::new();
        buffer.resize(expected_len_rgba8);

        // TOO potential optimization here: do create_new instead of from_data, since the buffer is empty anyway
        // Image::create_empty(width, height, use_mipmaps, format)
        let image =
            Image::create_from_data(width as i32, height as i32, false, Format::RGBA8, &buffer)
                .unwrap();
        let texture = ImageTexture::create_from_image(&image).unwrap();

        Self {
            image,
            texture,
            buffer,
            width,
            height,
        }
    }

    /// Use this to mutate the pixel buffer directly.
    /// Call `upload()` afterwards to push the changes to the GPU.
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.buffer.as_mut_slice()
    }

    /// Pushes the current contents of `buffer` to `ImageTexture` on the GPU.
    pub fn upload(&mut self) {
        self.image.set_data(
            self.width as i32,
            self.height as i32,
            false, // no mipmaps
            Format::RGBA8,
            &self.buffer,
        );

        self.texture.update(&self.image); // NOTE - The new image dimensions, format, and mipmaps configuration should match the existing texture's image configuration.
    }

    /// Helper method to write into the buffer and upload in one call.
    pub fn write_and_upload(&mut self, writer: impl FnOnce(&mut [u8])) {
        writer(self.buffer.as_mut_slice());
        self.upload();
    }

    /// Helper method copy a full buffer and upload it.
    /// Warning - panics if the source buffer has the wrong size!
    /// Deal with resizing plots carefully.
    pub fn update_and_upload(&mut self, source: &[u8]) {
        self.write_and_upload(|slice| slice.copy_from_slice(source));
    }

    /// Same as above excepts converts RGB8 -> RGBA8
    /// Should be faster than Godot's internal conversion machinery, because it avoids creating/allocating a new Image for the conversion step every frame
    fn update_and_upload_rgb8(&mut self, source_rgb: &[u8]) {
        // note - this assertion only works in debug mode
        debug_assert_eq!(source_rgb.len(), (self.width * self.height * 3) as usize);

        let dst = self.buffer.as_mut_slice();
        for (src_px, dst_px) in source_rgb.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
            dst_px[0] = src_px[0];
            dst_px[1] = src_px[1];
            dst_px[2] = src_px[2];
            dst_px[3] = 255;
        }
        self.upload();
    }
}

pub(crate) type PlotTextureCacheSlot = Rc<RefCell<(StreamingTexture, Instant, NotNan<f32>)>>;
//                                                                   ^^^^^^^ store Instant separately from the wait time
// RefCell needed since there is no get_mut(). Yikes!
// Otherwise, you cannot change the wait time while you're waiting.
// So if you do :line plot --rate 0.000001 you have to actually wait 11 days for the rate limit to expire.

fn get_or_create_texture(
    w: u32,
    h: u32,
    key: PlotAction,
    now: Instant,
    rate: NotNan<f32>,
    mut console: AsyncGd<CrabConsole>, // TODO maybe just pass the cache instead of the console, but that requires making it a Rc<RefCell<Cache>> probably because we need &mut
) -> PlotTextureCacheSlot {
    // Using the texture cache makes get_or_create_texture about 1.78x faster (n=~500)
    // So 0.39ms -> 0.21ms on average
    // On PC it was 2.4x faster (n=~2000)
    // So 0.27ms -> 0.11ms
    // Huge improvement!
    if !PLOT_USE_TEXTURE_CACHE_FLAG.get() {
        // not using the cache, so create a new texture every frame (slow).
        // (this will convert rgb8 -> rgba8 btw in Rust)
        return Rc::new(RefCell::new((StreamingTexture::new(w, h), now, nn(0.0))));
    }

    // try to get texture from cache
    {
        let mut console = console.bind_mut();
        let entry = console.plot_texture_cache.get(&key);
        if let Some(entry) = entry {
            // return it - update it later
            return Rc::clone(entry);
        }
    }

    // no value in the cache - create a new one
    let new_entry = Rc::new(RefCell::new((
        StreamingTexture::new(w, h),
        now, // TODO `now` will be slightly off here, since StreamingTexture::new takes some time to generate
        rate,
    )));
    let new_texture = Gd::clone(&new_entry.borrow().0.texture); // fast clone
    console
        .bind_mut()
        .plot_texture_cache
        .insert(key, Rc::clone(&new_entry));

    {
        let size = new_texture.get_size();
        let memory_usage_mb = if new_texture.get_format() == Format::RGBA8 {
            Some((size.x * size.y * 4.0).round() / 1024.0 / 1024.)
        } else {
            None // don't feel like implementing this logic for like 50 image formats
        };

        // keep this info statement here - it serves as a warning for when you accidentally create a lot of plots every frame
        tracing::info!(
            %new_texture,
            %size,
            vram_usage = %memory_usage_mb
                .map(|mb| format!("{:.2} MB", mb))
                .unwrap_or_else(|| "???".into()),
            "created fresh new plot texture",

        );
    }

    new_entry
}

fn _write_texture_cache_metrics(dur: Duration) {
    let map = indexmap! { "time" => AValue::from(dur.as_secs_f64())};
    let method = if PLOT_USE_TEXTURE_CACHE_FLAG.get() {
        "CACHE"
    } else {
        "NOCACHE"
    };
    let _ = append(&map, |date| {
        format!("out/profiling/{date}_get_or_create_texture[{method}].csv")
    })
    .tap_err(|err| tracing::warn!(?err));
}

#[cfg(test)]
mod tests {
    //use super::*;
    use crate::gd::console::eval::plot_draw::Metrics;

    const EPS: f32 = 1e-4;

    fn frames(ys: &[f32]) -> Vec<(f32, f32)> {
        ys.iter().enumerate().map(|(i, &y)| (i as f32, y)).collect()
    }

    #[test]
    fn nan_input_propagates() {
        let data: Vec<(f32, f32)> = frames(&[f32::NAN; 1]);
        let m = Metrics::new(&data).unwrap();
        assert!(m.min.is_nan());
        assert!(m.max.is_nan());
        assert!(m.avg.is_nan());
        assert!(m.last.is_nan());
        assert!(m.p95.is_nan());
        assert!(m.std_dev.is_nan());
        assert!(m.cv.unwrap().is_nan());
    }

    #[test]
    fn inf_input_propagates() {
        let data: Vec<(f32, f32)> = frames(&[f32::INFINITY; 1]);
        let m = Metrics::new(&data).unwrap();
        assert!(m.min.is_infinite());
        assert!(m.max.is_infinite());
        assert!(m.avg.is_infinite());
        assert!(m.last.is_infinite());
        assert!(m.p95.is_infinite());
        assert!(m.std_dev.is_nan()); // weird
        assert!(m.cv.unwrap().is_nan()); // weird
    }

    #[test]
    fn empty_input_returns_none() {
        let data: Vec<(f32, f32)> = vec![];
        assert_eq!(Metrics::new(&data), None);
    }

    #[test]
    fn single_sample() {
        let data = frames(&[16.0]);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.min, 16.0);
        assert_eq!(m.max, 16.0);
        assert_eq!(m.avg, 16.0);
        assert_eq!(m.last, 16.0);
        assert_eq!(m.p95, 16.0);
        assert_eq!(m.std_dev, 0.0);
        assert_eq!(m.cv.unwrap(), 0.0);
    }

    #[test]
    fn constant_values_have_zero_std_dev_and_cov() {
        let data = frames(&[16.6; 50]);
        let m = Metrics::new(&data).unwrap();

        assert_eq!(m.min, 16.6);
        assert_eq!(m.max, 16.6);
        assert!((m.avg - 16.6).abs() < EPS); // avg should be approximately 0
        assert!(m.std_dev < EPS, "expected ~0, got {}", m.std_dev);
        assert!(m.cv.unwrap() < EPS, "expected ~0, got {}", m.cv.unwrap());
        assert_eq!(m.p95, 16.6);
    }

    #[test]
    fn last_is_the_last_data_point_not_the_max() {
        let data = frames(&[10.0, 20.0, 5.0]);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.last, 5.0);
        assert_eq!(m.min, 5.0);
        assert_eq!(m.max, 20.0);
    }

    #[test]
    fn min_max_avg_basic() {
        let data = frames(&[16.0, 17.0, 16.0, 18.0, 15.0]);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.min, 15.0);
        assert_eq!(m.max, 18.0);
        assert!((m.avg - 16.4).abs() < EPS);
    }

    #[test]
    fn std_dev_matches_hand_calculation() {
        // data: 16, 17, 16, 18, 15 -> mean 16.4
        // population variance = 5.2 / 5 = 1.04 -> std_dev ~= 1.0198039
        let data = frames(&[16.0, 17.0, 16.0, 18.0, 15.0]);
        let m = Metrics::new(&data).unwrap();
        let expected_stddev: f32 = 1.0198039;
        assert!(
            (m.std_dev - expected_stddev).abs() < EPS,
            "expected {expected_stddev}, got {}",
            m.std_dev
        );
    }

    #[test]
    fn cov_is_std_dev_over_avg() {
        let data = frames(&[16.0, 17.0, 16.0, 18.0, 15.0]);
        let m = Metrics::new(&data).unwrap();
        assert!((m.cv.unwrap() - (m.std_dev / m.avg)).abs() < EPS);
    }

    #[test]
    fn p95_on_twenty_ascending_samples() {
        let ys: Vec<f32> = (1..=20).map(|i| i as f32).collect();
        let data = frames(&ys);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.p95, 19.0);
    }

    #[test]
    fn p95_is_order_independent() {
        let ys = [
            5.0, 20.0, 1.0, 15.0, 3.0, 19.0, 2.0, 18.0, 4.0, 17.0, 6.0, 16.0, 7.0, 14.0, 8.0, 13.0,
            9.0, 12.0, 10.0, 11.0,
        ];
        let data = frames(&ys);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.p95, 19.0);
    }

    #[test]
    fn small_sample_p95_is_max_value() {
        // small sample size means p95 == max (index gets rounded up)
        let ys: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        let data = frames(&ys);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.p95, m.max);
    }

    #[test]
    fn handles_spike_without_panicking_and_reflects_in_max_and_std_dev() {
        let mut ys = vec![16.6; 99];
        ys.push(200.0);
        let data = frames(&ys);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.max, 200.0);
        assert!(m.std_dev > 1.0, "spike should inflate std_dev noticeably");
        assert_eq!(m.p95, 16.6); // 1 of 100 frames -> spike stays outside top 5%
    }

    #[test]
    fn last_element_preserved_even_after_internal_sort() {
        // Regression check: ensures sort comes AFTER first() properly
        let data = frames(&[3.0, 1.0, 2.0]);
        let m = Metrics::new(&data).unwrap();
        assert_eq!(m.last, 2.0);
    }
}
