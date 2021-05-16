use nats;
use std::{io, num::NonZeroUsize, thread, time::Duration, time::Instant};

use historian::Histo;
use rand::prelude::*;
use rand_distr::WeightedAliasIndex;
use spinners::{Spinner, Spinners};
use structopt::StructOpt;

use crossterm::style::Styler;

use tui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{BarChart, Block, Borders},
    Terminal,
};

#[derive(Debug, StructOpt)]
struct Args {
    /// The NATS server. Defaults to local but will fallback to demo.nats.io.
    #[structopt(long, short, default_value = "127.0.0.1")]
    server: String,

    /// Number of service responders.
    #[structopt(long, short = "w", default_value = "1")]
    num_responders: NonZeroUsize,

    /// Number of replicated responses.
    #[structopt(long, short = "r", default_value = "1")]
    num_replicas: NonZeroUsize,

    /// Number of service requests.
    #[structopt(long, short = "n", default_value = "100")]
    num_requests: NonZeroUsize,
}

// Exponential delays in ms for our workers.
const DELAYS: [(u64, u64); 5] = [(5, 65), (10, 25), (15, 4), (50, 3), (100, 3)];

lazy_static::lazy_static! {
    static ref HISTOGRAM: Histo = Default::default();
    static ref DIST: WeightedAliasIndex<u64> = WeightedAliasIndex::new(DELAYS.iter().map(|item| item.1).collect()).unwrap();
}

fn main() -> io::Result<()> {
    let args = Args::from_args();

    // Connect to the NATS network.
    // This is like your computer connecting to WiFi or your phone connecting to the cellular network.
    println!("Attempting to connect to NATS [{}]", &args.server);
    let nc = nats::connect(&args.server).unwrap_or_else(|_| {
        println!("Falling back to [demo.nats.io]");
        nats::connect("demo.nats.io").unwrap()
    });
    let rtt = nc.rtt()?;

    println!();
    println!("{}:        {:?}", "RTT".bold(), rtt);
    println!("{}: {:?}", "Responders".bold(), args.num_responders);
    println!("{}: {:?}", "Duplicated".bold(), args.num_replicas);
    println!("{}:   {:?}", "Requests".bold(), args.num_requests);
    println!();

    // Spin up our subscriptions/workers.
    // Pick an inbox in case we use something public like demo.nats.io.
    let svc_addr = nc.new_inbox();

    for i in 0..args.num_replicas.get() {
        let qg = format!("qg:{}", i);
        for _ in 0..args.num_responders.get() {
            // This is our service.
            nc.queue_subscribe(&svc_addr, &qg)?
                .with_handler(move |msg| {
                    thread::sleep(Duration::from_millis(
                        DELAYS[DIST.sample(&mut thread_rng())].0,
                    ));
                    msg.respond("42")
                });
        }
    }

    let tx_time: Duration = 2 * rtt;
    let calc_elapsed = |start: Instant| {
        let mut elapsed = start.elapsed();
        if elapsed > tx_time {
            elapsed -= tx_time;
        }
        elapsed.as_millis()
    };

    // Send requests.
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.hide_cursor()?;
    let num_reqs = args.num_requests.get();
    let sp = Spinner::new(Spinners::Dots9, format!("Sending {} requests", num_reqs));

    for _ in 0..num_reqs {
        let start = Instant::now();
        nc.request(&svc_addr, "Hello World")?;
        HISTOGRAM.measure(calc_elapsed(start) as f64);
    }

    sp.stop();
    terminal.show_cursor()?;

    // Gather the results.
    let mut data: Vec<(&str, u64)> = Vec::new();
    data.push(("  min", HISTOGRAM.percentile(0.0) as u64));
    for pctl in &[50., 75., 90., 95., 99.0] {
        let p = HISTOGRAM.percentile(*pctl);
        let l = format!("{:4}th", pctl);
        data.push((Box::leak(l.into_boxed_str()), p as u64));
    }
    data.push(("  max", HISTOGRAM.percentile(100.0) as u64));

    let barchart = BarChart::default()
        .block(Block::default().title(" Latency(ms)").borders(Borders::ALL))
        .data(&data)
        .max(110)
        .bar_style(Style::default().fg(Color::Gray))
        .bar_width(7)
        .label_style(Style::default().add_modifier(Modifier::BOLD))
        .value_style(Style::default().fg(Color::Black).bg(Color::Gray));

    const W: u16 = 58;
    const H: u16 = 11;

    // Hack to make sure we have room if near bottom of terminal.
    // I am sure there is a much better way to do this.. Not a tui expert.
    print!("{}", "\n".repeat(H.into()));
    let (_, y) = terminal.get_cursor()?;
    let area = Rect::new(0, y - H, W, H);

    terminal.draw(|f| {
        f.render_widget(barchart, area);
    })?;

    println!("\n");
    Ok(())
}
