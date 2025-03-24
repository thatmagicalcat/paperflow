use std::ptr;

use x11::xlib;

use ffmpeg_next as ffmpeg;

use ffmpeg::format::{Pixel, input};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;

fn main() {
    let (fps, mut video_frames) = load_video("video.mp4").unwrap();
    let frame_time = std::time::Duration::from_millis(((1.0 / fps) * 1000.0) as _);

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

    let ximgs = video_frames
        .iter_mut()
        .map(|frame| unsafe {
            xlib::XCreateImage(
                display,
                visual.as_mut(),
                depth as _,
                xlib::ZPixmap,
                0,
                frame.as_mut_ptr() as _,
                width as _,
                height as _,
                32,
                0,
            )
        })
        .collect::<Vec<_>>();

    let pixmap = unsafe { xlib::XCreatePixmap(display, root, width as _, height as _, depth as _) };

    if pixmap == 0 {
        panic!("failed to create pixmap");
    }

    let gc = unsafe { xlib::XCreateGC(display, root, 0, ptr::null_mut()) };

    let mut frame_index = 0;
    loop {
        if frame_index >= video_frames.len() {
            // frame_index = 0;
            break;
        }

        let ximg = ximgs[frame_index];

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

        frame_index += 1;

        std::thread::sleep(frame_time);
    }

    unsafe {
        // causes double free
        // xlib::XDestroyImage(ximg.as_mut());

        xlib::XFreePixmap(display, pixmap);
        xlib::XFreeGC(display, gc);
        xlib::XCloseDisplay(display);
    };
}

fn load_video(path: impl Into<String>) -> Result<(f32, Vec<Vec<u8>>), ffmpeg::Error> {
    ffmpeg::init()?;

    println!("[Info] Loading video");

    let path = path.into();
    let (tx, rx) = std::sync::mpsc::channel();

    let thread = std::thread::spawn(move || -> Result<f32, ffmpeg::Error> {
        let Ok(mut ictx) = input(&path) else {
            panic!();
        };

        let input = ictx
            .streams()
            .best(Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;

        let frame_rate = input.avg_frame_rate();
        let frame_rate = frame_rate.numerator() as f32 / frame_rate.denominator() as f32;

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

        let mut receive_and_process_decoded_frames =
            move |decoder: &mut ffmpeg::decoder::Video| -> Result<(), ffmpeg::Error> {
                let mut decoded = Video::empty();
                while decoder.receive_frame(&mut decoded).is_ok() {
                    let mut rgb_frame = Video::empty();
                    scaler.run(&decoded, &mut rgb_frame)?;
                    tx.send(Vec::from(rgb_frame.data(0))).unwrap();
                }

                Ok(())
            };

        for (stream, packet) in ictx.packets() {
            if stream.index() == video_stream_index {
                decoder.send_packet(&packet)?;
                receive_and_process_decoded_frames(&mut decoder)?;
            }
        }

        decoder.send_eof()?;
        receive_and_process_decoded_frames(&mut decoder)?;

        Ok(frame_rate)
    });

    let mut frames = vec![];
    while let Ok(frame) = rx.recv() {
        frames.push(frame);
    }

    let frame_rate = thread.join().unwrap()?;
    println!("[Info] Loaded {} frames, {frame_rate:.1} FPS", frames.len());
    println!(
        "[Info] Size: {:0.2} GB",
        (frames.len() * frames[0].len() * std::mem::size_of::<u8>()) as f32
            / 1024.0 // KB
            / 1024.0 // MB
            / 1024.0 // GB
    );

    Ok((frame_rate, frames))
}
