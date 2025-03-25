use std::ptr;

use x11::xlib;

use ffmpeg_next as ffmpeg;

use ffmpeg::format::{Pixel, input};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;

fn main() {
    println!("[Info] Loading video");
    let mut frames = load_video("video.mp4").unwrap();
    println!("[Info] Loaded");

    let display = unsafe { xlib::XOpenDisplay(std::ptr::null()) };

    if display.is_null() {
        panic!("failed to open display");
    }

    let screen = unsafe { xlib::XDefaultScreen(display) };
    let root = unsafe { xlib::XRootWindow(display, screen) };

    let height = unsafe { xlib::XDisplayHeight(display, screen) };
    let width = unsafe { xlib::XDisplayWidth(display, screen) };

    let depth = unsafe { xlib::XDefaultDepth(display, screen) };
    let mut visual = ptr::NonNull::new(unsafe { xlib::XDefaultVisual(display, screen) }).unwrap();

    unsafe {
        xlib::XSetWindowBackground(display, root, xlib::XBlackPixel(display, screen));
        xlib::XClearWindow(display, root);
    }

    let pixmap = unsafe { xlib::XCreatePixmap(display, root, width as _, height as _, depth as _) };

    if pixmap == 0 {
        panic!("failed to create pixmap");
    }

    let gc = unsafe { xlib::XCreateGC(display, root, 0, ptr::null_mut()) };

    loop {
        // this function blocks so the wallpaper is updated 24 times a second
        let mut frame = frames.next().unwrap();
        let ximg = unsafe {
            xlib::XCreateImage(
                display,
                visual.as_mut(),
                depth as _,
                xlib::ZPixmap,
                0,
                frame.data_mut(0).as_mut_ptr() as _,
                width as _,
                height as _,
                32,
                0,
            )
        };

        unsafe {
            xlib::XPutImage(
                display,
                pixmap,
                gc,
                ximg,
                0,
                0,
                0,
                0,
                width as _,
                height as _,
            );

            xlib::XCopyArea(
                display,
                pixmap,
                root,
                gc,
                0,
                0,
                width as _,
                height as _,
                0,
                0,
            );

            xlib::XFlush(display);
        }
    }
}

fn load_video(
    path: impl Into<String>,
) -> Result<impl Iterator<Item = ffmpeg::frame::Video>, ffmpeg::Error> {
    ffmpeg::init()?;

    let path = path.into();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || -> Result<f32, ffmpeg::Error> {
        let Ok(mut ictx) = input(&path) else {
            panic!();
        };

        let input = ictx
            .streams()
            .best(Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;

        let frame_rate = input.avg_frame_rate();
        let frame_rate = frame_rate.numerator() as f32 / frame_rate.denominator() as f32;
        let frame_time = std::time::Duration::from_millis(((1.0 / frame_rate) * 1000.0) as _);

        let video_stream_index = input.index();

        let context_decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
        let mut decoder = context_decoder.decoder().video()?;

        let mut scaler = Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::BGRA,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )?;

        let mut last_frame = None;
        loop {
            for (stream, packet) in ictx.packets() {
                if stream.index() == video_stream_index {
                    decoder.send_packet(&packet)?;

                    // send the last rendered frame to the main thread
                    if let Some(last_frame) = last_frame.take() {
                        tx.send(last_frame).unwrap();
                    }

                    // render the next frame
                    let mut decoded = Video::empty();
                    while decoder.receive_frame(&mut decoded).is_ok() {
                        let mut rgb_frame = Video::empty();
                        scaler.run(&decoded, &mut rgb_frame)?;
                        last_frame = Some(rgb_frame);
                    }

                    std::thread::sleep(frame_time);
                }
            }

            // restart the video
            ictx.seek(0, ..)?;
        }
    });

    Ok(std::iter::from_fn(move || rx.recv().ok()))
}
