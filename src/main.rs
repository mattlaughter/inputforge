mod app;
mod config;
mod desktop;
mod devices;
mod engine;
mod hardware;
mod keys;
mod lighting;
mod output;
mod selftest;
mod theme;
mod tray;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-devices") {
        let r = devices::scan();
        for d in r.devices {
            println!(
                "{:<22} {:<18} {} [{}]",
                d.path.display(),
                d.kind.label(),
                d.name,
                d.phys
            );
        }
        if r.unreadable > 0 {
            eprintln!("({} devices unreadable — permissions)", r.unreadable);
            let hint = devices::group_hint();
            if !hint.is_empty() {
                eprintln!("{hint}");
            }
        }
        println!("uinput writable: {}", devices::uinput_writable());
        return Ok(());
    }

    if args.iter().any(|a| a == "--apply-lighting") {
        let cfg = config::Config::load().unwrap_or_default();
        for line in lighting::apply_now(&cfg.lighting) {
            println!("{line}");
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--selftest") {
        match selftest::run() {
            Ok(true) => println!("All checks passed."),
            Ok(false) => std::process::exit(1),
            Err(e) => {
                eprintln!("selftest error: {e}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    run_gui(args.iter().any(|a| a == "--background"))
}

/// One long-lived process: tray + engine + lighting stay up while the window
/// is opened and closed. `--background` starts with only the tray.
fn run_gui(background: bool) -> eframe::Result {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    let (tx, rx) = std::sync::mpsc::channel();
    let waker = tray::Waker::default();
    let _bus = match tray::claim(tx.clone(), waker.clone(), (!background).then_some("Show")) {
        tray::Instance::Primary(conn) => conn,
        tray::Instance::Secondary => {
            if !background {
                eprintln!("InputForge is already running — showing its window.");
            }
            return Ok(());
        }
    };

    let app = Rc::new(RefCell::new(app::App::new(waker, rx)));
    app.borrow_mut().start_tray(tx);

    // Background start without a tray host would leave nothing visible:
    // open the window instead.
    let mut show = !background || !app.borrow().has_tray();
    loop {
        if show {
            let options = eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("InputForge")
                    .with_app_id("inputforge")
                    .with_icon(std::sync::Arc::new(tray::window_icon()))
                    .with_inner_size([980.0, 680.0])
                    .with_min_inner_size([720.0, 480.0]),
                run_and_return: true,
                ..Default::default()
            };
            let a = app.clone();
            eframe::run_native(
                "InputForge",
                options,
                Box::new(move |_cc| Ok(Box::new(app::Window(a)))),
            )?;
            app.borrow_mut().detach();
            // Without a tray there's no way back to the window: quit.
            if !app.borrow().has_tray() {
                break;
            }
        }
        if app.borrow().wants_quit() {
            break;
        }
        show = app.borrow_mut().wait(Duration::from_millis(500));
        if app.borrow().wants_quit() {
            break;
        }
    }
    app.borrow_mut().shutdown();
    Ok(())
}
