mod app;
mod config;
mod desktop;
mod devices;
mod engine;
mod hardware;
mod keys;
mod lighting;
mod output;
mod razer;
mod selftest;
mod theme;
mod tray;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-devices") {
        let r = devices::scan();
        for d in r.devices {
            println!(
                "{:<22} {:<18} {:04x}:{:04x} {:<9} {} [{}]",
                d.path.display(),
                d.kind.label(),
                d.vendor,
                d.product,
                if d.accessible { "access" } else { "no-access" },
                d.name,
                d.phys
            );
        }
        if r.unreadable > 0 {
            eprintln!(
                "({} devices not accessible; turn a device on in InputForge to grant access to it)",
                r.unreadable
            );
        }
        let hint = devices::access_hint();
        if !hint.is_empty() {
            eprintln!("{hint}");
        }
        println!("uinput writable: {}", devices::uinput_writable());
        return Ok(());
    }

    // Root helper (run through pkexec by the app): per-device uaccess rules.
    for (flag, allow) in [("--udev-allow", true), ("--udev-revoke", false)] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            let ids: Option<Vec<(u16, u16)>> =
                args[i + 1..].iter().map(|a| devices::parse_id(a)).collect();
            let Some(ids) = ids.filter(|v| !v.is_empty()) else {
                eprintln!("usage: inputforge {flag} vvvv:pppp [vvvv:pppp ...]");
                std::process::exit(2);
            };
            let r = if allow {
                devices::udev_update(&ids, &[])
            } else {
                devices::udev_update(&[], &ids)
            };
            match r {
                Ok(now) => {
                    let list: Vec<String> = now
                        .iter()
                        .map(|(v, p)| format!("{v:04x}:{p:04x}"))
                        .collect();
                    println!(
                        "InputForge device access: {}",
                        if list.is_empty() {
                            "none".into()
                        } else {
                            list.join(" ")
                        }
                    );
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("inputforge {flag}: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    if args.iter().any(|a| a == "--lighting-info") {
        for l in lighting::describe_hidraw() {
            println!("{l}");
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--apply-lighting") {
        let cfg = config::Config::load().unwrap_or_default();
        for line in lighting::apply_now(&cfg.lighting) {
            println!("{line}");
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--selftest") && unsafe { libc::geteuid() } != 0 {
        // The test reads InputForge's own virtual devices, which (by design)
        // ordinary programs can't. Re-run as root through a password prompt.
        eprintln!(
            "The self-test reads InputForge's virtual devices and needs admin rights; asking…"
        );
        let st = std::process::Command::new("pkexec")
            .arg(std::env::current_exe().expect("exe path"))
            .arg("--selftest")
            .status();
        std::process::exit(st.ok().and_then(|s| s.code()).unwrap_or(1));
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
