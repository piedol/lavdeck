//! Plays one take's media with AVPlayer, as `TakePlayer.swift`. Raw screen and camera
//! recordings have no mic audio, so they play as a composition with the take's audio placed
//! at the offset Lavdeck recorded. The playhead ticks ten times a second; only the parts of
//! Recordings that show it (the scrubber, the transcript highlight) redraw on a tick.

use std::cell::RefCell;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAsset, AVMediaTypeAudio, AVMediaTypeVideo, AVMutableComposition, AVPlayer, AVPlayerItem,
    AVPlayerItemStatus, AVPlayerTimeControlStatus, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange, kCMPersistentTrackID_Invalid};

use crate::i18n::tr;
use crate::model::takes::{MediaKind, Take, TakeMedia};
use crate::util;

pub struct Player {
    pub av: Retained<AVPlayer>,
    _observer: Retained<AnyObject>,
    pub media: Option<TakeMedia>,
    take_id: Option<String>,
    pub playing: bool,
    pub duration: f64,
    pub time: f64,
    /// Why the loaded media can't play (moved, deleted or unreadable).
    pub error: Option<String>,
    /// Bumped per load, so a slow load can't land after a newer one.
    generation: u64,
}

thread_local! {
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

fn cm(seconds: f64) -> CMTime {
    // SAFETY: plain value constructor.
    unsafe { CMTime::with_seconds(seconds, 600) }
}

fn secs(t: CMTime) -> f64 {
    // SAFETY: plain value accessor.
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}

fn make() -> Player {
    let mtm = util::mtm();
    // SAFETY: a new, empty player.
    let av = unsafe { AVPlayer::new(mtm) };
    let tick = RcBlock::new(|t: CMTime| {
        let time = secs(t);
        let changed = PLAYER.with(|p| {
            let mut p = p.borrow_mut();
            let Some(p) = p.as_mut() else { return false };
            if p.media.is_some() {
                p.time = time;
            }
            // SAFETY: plain property reads on the main thread.
            let (status, item_duration) = unsafe {
                (
                    p.av.timeControlStatus(),
                    p.av.currentItem().map(|i| secs(i.duration())),
                )
            };
            let mut changed = false;
            if let Some(d) = item_duration
                && d > 0.0
                && (d - p.duration).abs() > 0.01
            {
                p.duration = d;
                changed = true;
            }
            let playing = status != AVPlayerTimeControlStatus::Paused;
            if playing != p.playing {
                p.playing = playing;
                changed = true;
            }
            changed
        });
        if changed {
            crate::recordings::player_changed();
        }
        crate::recordings::clock_tick();
    });
    // SAFETY: the main queue; the observer is removed with the player (it lives for the app).
    let observer = unsafe {
        av.addPeriodicTimeObserverForInterval_queue_usingBlock(
            cm(0.1),
            Some(&dispatch2::DispatchQueue::main()),
            &tick,
        )
    };
    Player {
        av,
        _observer: observer,
        media: None,
        take_id: None,
        playing: false,
        duration: 0.0,
        time: 0.0,
        error: None,
        generation: 0,
    }
}

/// Reads or changes the player (made on first use).
pub fn with<R>(f: impl FnOnce(&mut Player) -> R) -> R {
    PLAYER.with(|p| {
        let mut p = p.borrow_mut();
        f(p.get_or_insert_with(make))
    })
}

pub fn av() -> Retained<AVPlayer> {
    with(|p| p.av.clone())
}

/// Loads `media` from `take`, keeping the playhead when switching between a take's sources.
pub fn load(media: &TakeMedia, take: &Take, autoplay: bool) {
    let same = with(|p| p.media.as_ref() == Some(media) && p.take_id.as_deref() == Some(&take.id));
    if same {
        if autoplay {
            // SAFETY: plain call.
            with(|p| unsafe { p.av.play() });
        }
        return;
    }
    let (keep, start, generation) = with(|p| {
        let same_take = p.take_id.as_deref() == Some(&take.id);
        let keep = if same_take { p.time } else { 0.0 };
        let start = autoplay || (same_take && p.playing);
        // Stop the previous take now rather than when the new one has loaded.
        if !same_take {
            // SAFETY: plain call.
            unsafe { p.av.pause() };
        }
        p.generation += 1;
        p.media = Some(media.clone());
        p.take_id = Some(take.id.clone());
        p.error = None;
        p.duration = take.duration.unwrap_or(0.0);
        p.time = keep;
        (keep, start, p.generation)
    });
    crate::recordings::player_changed();
    let asset = asset(media, take);
    // SAFETY: a valid asset, on the main thread.
    let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, util::mtm()) };
    // SAFETY: plain call.
    with(|p| unsafe { p.av.replaceCurrentItemWithPlayerItem(Some(&item)) });
    wait_ready(item, generation, keep, start, 0);
}

/// A paused player shows black until a frame is decoded: wait until the item is ready, then
/// seek once so the first (or current) frame shows without pressing Play.
fn wait_ready(item: Retained<AVPlayerItem>, generation: u64, keep: f64, start: bool, tries: u32) {
    if with(|p| p.generation) != generation {
        return;
    }
    // SAFETY: plain property read.
    let status = unsafe { item.status() };
    if status == AVPlayerItemStatus::Unknown && tries < 400 {
        util::later(0.015, move || {
            wait_ready(item, generation, keep, start, tries + 1)
        });
        return;
    }
    if status != AVPlayerItemStatus::ReadyToPlay {
        // SAFETY: plain property read.
        let reason = unsafe { item.error() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| tr("This file can’t be played."));
        with(|p| p.error = Some(reason));
        crate::recordings::player_changed();
        return;
    }
    with(|p| {
        // SAFETY: plain calls on the main thread.
        unsafe {
            p.av.seekToTime_toleranceBefore_toleranceAfter(cm(keep), cm(0.0), cm(0.0));
            if start {
                p.av.play();
            }
        }
    });
}

pub fn toggle() {
    // SAFETY: plain calls.
    with(|p| unsafe { if p.playing { p.av.pause() } else { p.av.play() } });
}

pub fn play_from(seconds: f64) {
    with(|p| {
        // SAFETY: plain calls.
        unsafe {
            p.av.seekToTime_toleranceBefore_toleranceAfter(cm(seconds), cm(0.0), cm(0.0));
            p.av.play();
        }
        p.time = seconds;
    });
    crate::recordings::clock_tick();
}

pub fn seek(seconds: f64) {
    with(|p| {
        // SAFETY: plain call.
        unsafe {
            p.av.seekToTime_toleranceBefore_toleranceAfter(cm(seconds), cm(0.0), cm(0.0))
        };
        p.time = seconds;
    });
    crate::recordings::clock_tick();
}

pub fn stop() {
    with(|p| {
        p.generation += 1;
        // SAFETY: plain calls.
        unsafe {
            p.av.pause();
            p.av.replaceCurrentItemWithPlayerItem(None);
        }
        p.media = None;
        p.take_id = None;
        p.error = None;
        p.time = 0.0;
        p.playing = false;
    });
    crate::recordings::player_changed();
}

/// The file itself, or a raw capture lined up with the take's audio.
fn asset(media: &TakeMedia, take: &Take) -> Retained<AVAsset> {
    let url = util::file_url(&media.file.path);
    // SAFETY: a file URL and no options.
    let plain = || -> Retained<AVAsset> {
        Retained::into_super(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) })
    };
    let raw = matches!(media.kind, MediaKind::Screen | MediaKind::Camera);
    let Some(audio) = take.main_audio().filter(|_| raw) else {
        return plain();
    };
    // SAFETY: AVFoundation's composition calls on local files, with valid tracks and ranges.
    unsafe {
        let (Some(audio_type), Some(video_type)) = (AVMediaTypeAudio, AVMediaTypeVideo) else {
            return plain();
        };
        let audio_asset = AVURLAsset::URLAssetWithURL_options(&util::file_url(&audio.path), None);
        let video_asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        #[allow(deprecated)]
        let (Some(audio_track), Some(video_track)) = (
            audio_asset.tracksWithMediaType(audio_type).firstObject(),
            video_asset.tracksWithMediaType(video_type).firstObject(),
        ) else {
            return plain();
        };
        #[allow(deprecated)]
        let (audio_len, video_len) = (secs(audio_asset.duration()), secs(video_asset.duration()));
        let composition = AVMutableComposition::composition();
        if let Some(t) = composition
            .addMutableTrackWithMediaType_preferredTrackID(audio_type, kCMPersistentTrackID_Invalid)
        {
            let _ = t.insertTimeRange_ofTrack_atTime_error(
                CMTimeRange::new(cm(0.0), cm(audio_len)),
                &audio_track,
                cm(0.0),
            );
        }
        // Time zero is the take's first audio sample.
        let offset = media.file.offset.unwrap_or(0.0);
        let source_start = (-offset).max(0.0);
        let at = offset.max(0.0);
        let length = (video_len - source_start).min(audio_len - at);
        if length > 0.0
            && let Some(t) = composition.addMutableTrackWithMediaType_preferredTrackID(
                video_type,
                kCMPersistentTrackID_Invalid,
            )
        {
            let _ = t.insertTimeRange_ofTrack_atTime_error(
                CMTimeRange::new(cm(source_start), cm(length)),
                &video_track,
                cm(at),
            );
            #[allow(deprecated)]
            t.setPreferredTransform(video_track.preferredTransform());
        }
        Retained::into_super(Retained::into_super(composition))
    }
}

/// The view model for the transcript highlight: the line under the playhead, once playback
/// has started (so a freshly loaded take doesn't look like it's playing line one).
pub fn current_time_if_started() -> Option<f64> {
    with(|p| (p.media.is_some() && (p.playing || p.time > 0.0)).then_some(p.time))
}
