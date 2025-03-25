use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use x11::xlib;

use ffmpeg_next as ffmpeg;

use ffmpeg::format::{Pixel, input};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;

const REFRESH_RATE: f32 = 60.0;
const FRAME_TIME: Duration = Duration::from_millis(((1.0 / REFRESH_RATE) * 1000.0) as _);

fn main() {
    let display = unsafe { xlib::XOpenDisplay(std::ptr::null()) };

    if display.is_null() {
        panic!("failed to open display");
    }

    let screen = unsafe { xlib::XDefaultScreen(display) };
    let root = unsafe { xlib::XRootWindow(display, screen) };

    let height = unsafe { xlib::XDisplayHeight(display, screen) } as u32;
    let width = unsafe { xlib::XDisplayWidth(display, screen) } as u32;

    println!("[Info] Loading video");
    let frame_receiver = load_video("video.mp4", width, height).unwrap();
    println!("[Info] Loaded");

    let depth = unsafe { xlib::XDefaultDepth(display, screen) };
    let mut visual = ptr::NonNull::new(unsafe { xlib::XDefaultVisual(display, screen) }).unwrap();

    unsafe {
        xlib::XSetWindowBackground(display, root, xlib::XBlackPixel(display, screen));
        xlib::XClearWindow(display, root);
    }

    let pixmap = unsafe { xlib::XCreatePixmap(display, root, width, height, depth as _) };

    if pixmap == 0 {
        panic!("failed to create pixmap");
    }

    let gc = unsafe { xlib::XCreateGC(display, root, 0, ptr::null_mut()) };

    let mut create_ximg = |frame: &mut ffmpeg::frame::Video| unsafe {
        xlib::XCreateImage(
            display,
            visual.as_mut(),
            depth as _,
            xlib::ZPixmap,
            0,
            frame.data_mut(0).as_mut_ptr() as _,
            width,
            height,
            32,
            0,
        )
    };

    let mut last_frame = frame_receiver.recv().unwrap();
    let mut last_frame_ximg = create_ximg(&mut last_frame);

    loop {
        match frame_receiver.try_recv() {
            Ok(frame) => {
                last_frame = frame;
                last_frame_ximg = create_ximg(&mut last_frame);
            }

            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => break,
        }

        unsafe {
            xlib::XPutImage(
                display,
                pixmap,
                gc,
                last_frame_ximg,
                0,
                0,
                0,
                0,
                width,
                height,
            );

            xlib::XCopyArea(
                display,
                pixmap,
                root,
                gc,
                0,
                0,
                width ,
                height,
                0,
                0,
            );

            xlib::XFlush(display);
            std::thread::sleep(FRAME_TIME);
        }
    }

    unsafe {
        xlib::XFreePixmap(display, pixmap);
        xlib::XFreeGC(display, gc);
        xlib::XCloseDisplay(display);
    }
}

fn load_video(
    path: impl Into<String>,
    width: u32,
    height: u32,
) -> Result<mpsc::Receiver<ffmpeg::frame::Video>, ffmpeg::Error> {
    ffmpeg::init()?;

    let path = path.into();
    let (tx, rx) = mpsc::channel();

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
        dbg!(frame_rate);
        let frame_time = Duration::from_millis(((1.0 / frame_rate) * 1000.0) as _);

        let video_stream_index = input.index();

        let context_decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
        let mut decoder = context_decoder.decoder().video()?;

        let mut scaler = Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::BGRA,
            width,
            height,
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

    Ok(rx)
}
