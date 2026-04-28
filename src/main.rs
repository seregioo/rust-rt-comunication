mod comedi;
mod hindmarsh_rose;
mod rt_thread;

use libc::{CLOCK_MONOTONIC, clock_gettime, timespec};
use polars_core::prelude::*;
use polars_io::prelude::*;
use std::{
    collections::HashSet,
    env,
    fs::File,
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

use crate::comedi::comedi_driver::ComediDaq;
use crate::hindmarsh_rose::{
    HindmarshRoseModel, HindmarshRoseRungeKutta, ModelDerivativeVariables, ModelTemporalVariables,
};
use crate::rt_thread::ActiveRtBackend;

fn ns_to_us(ns: i64) -> f64 {
    ns as f64 / 1_000.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Trace,
    Calibrate,
}

#[derive(Debug, Clone)]
struct AppConfig {
    mode: RunMode,
    device_path: String,
    input_port: String,
    output_port: String,
    ai_range_index: u32,
    ao_range_index: u32,
    ai_aref: u32,
    ao_aref: u32,
    output_csv: String,
    target_cycle_us: f64,
    duration_s: f64,
    calibration_csv: String,
    calibration_reads: usize,
    calibration_settle_us: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: RunMode::Trace,
            device_path: "/dev/comedi0".to_string(),
            input_port: "a7".to_string(),
            output_port: "i0".to_string(),
            ai_range_index: 0,
            ao_range_index: 0,
            ai_aref: 0,
            ao_aref: 0,
            output_csv: "rust-test.csv".to_string(),
            target_cycle_us: 100.0,
            duration_s: 60.0 * 10.0,
            calibration_csv: "daq-calibration.csv".to_string(),
            calibration_reads: 64,
            calibration_settle_us: 2_000,
        }
    }
}

fn parse_f64(value: &str, flag: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|err| format!("Invalid value for {flag}: {err}"))
}

fn parse_u32(value: &str, flag: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|err| format!("Invalid value for {flag}: {err}"))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|err| format!("Invalid value for {flag}: {err}"))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|err| format!("Invalid value for {flag}: {err}"))
}

fn print_usage(binary_name: &str) {
    eprintln!(
        "Usage: {binary_name} [--mode trace|calibrate] [--device-path PATH] [--input-port a7] [--output-port i0] [--ai-range-index 0] [--ao-range-index 0] [--ai-aref 0] [--ao-aref 0] [--output-csv FILE] [--target-cycle-us 100] [--duration-s 600] [--calibration-csv FILE] [--calibration-reads 64] [--calibration-settle-us 2000]"
    );
}

fn parse_args() -> Result<AppConfig, String> {
    let mut config = AppConfig::default();
    let binary_name = env::args()
        .next()
        .unwrap_or_else(|| "rust-comunication".to_string());
    let mut args = env::args().skip(1);

    while let Some(flag) = args.next() {
        let value = match flag.as_str() {
            "--help" | "-h" => {
                print_usage(&binary_name);
                std::process::exit(0);
            }
            "--mode" => args
                .next()
                .ok_or_else(|| "Missing value for --mode".to_string())?,
            "--device-path" => args
                .next()
                .ok_or_else(|| "Missing value for --device-path".to_string())?,
            "--input-port" => args
                .next()
                .ok_or_else(|| "Missing value for --input-port".to_string())?,
            "--output-port" => args
                .next()
                .ok_or_else(|| "Missing value for --output-port".to_string())?,
            "--ai-range-index" => args
                .next()
                .ok_or_else(|| "Missing value for --ai-range-index".to_string())?,
            "--ao-range-index" => args
                .next()
                .ok_or_else(|| "Missing value for --ao-range-index".to_string())?,
            "--ai-aref" => args
                .next()
                .ok_or_else(|| "Missing value for --ai-aref".to_string())?,
            "--ao-aref" => args
                .next()
                .ok_or_else(|| "Missing value for --ao-aref".to_string())?,
            "--output-csv" => args
                .next()
                .ok_or_else(|| "Missing value for --output-csv".to_string())?,
            "--target-cycle-us" => args
                .next()
                .ok_or_else(|| "Missing value for --target-cycle-us".to_string())?,
            "--duration-s" => args
                .next()
                .ok_or_else(|| "Missing value for --duration-s".to_string())?,
            "--calibration-csv" => args
                .next()
                .ok_or_else(|| "Missing value for --calibration-csv".to_string())?,
            "--calibration-reads" => args
                .next()
                .ok_or_else(|| "Missing value for --calibration-reads".to_string())?,
            "--calibration-settle-us" => args
                .next()
                .ok_or_else(|| "Missing value for --calibration-settle-us".to_string())?,
            _ => {
                print_usage(&binary_name);
                return Err(format!("Unknown argument: {flag}"));
            }
        };

        match flag.as_str() {
            "--mode" => {
                config.mode = match value.as_str() {
                    "trace" => RunMode::Trace,
                    "calibrate" => RunMode::Calibrate,
                    _ => return Err("Expected --mode to be 'trace' or 'calibrate'".to_string()),
                };
            }
            "--device-path" => config.device_path = value,
            "--input-port" => config.input_port = value,
            "--output-port" => config.output_port = value,
            "--ai-range-index" => config.ai_range_index = parse_u32(&value, "--ai-range-index")?,
            "--ao-range-index" => config.ao_range_index = parse_u32(&value, "--ao-range-index")?,
            "--ai-aref" => config.ai_aref = parse_u32(&value, "--ai-aref")?,
            "--ao-aref" => config.ao_aref = parse_u32(&value, "--ao-aref")?,
            "--output-csv" => config.output_csv = value,
            "--target-cycle-us" => config.target_cycle_us = parse_f64(&value, "--target-cycle-us")?,
            "--duration-s" => config.duration_s = parse_f64(&value, "--duration-s")?,
            "--calibration-csv" => config.calibration_csv = value,
            "--calibration-reads" => {
                config.calibration_reads = parse_usize(&value, "--calibration-reads")?
            }
            "--calibration-settle-us" => {
                config.calibration_settle_us = parse_u64(&value, "--calibration-settle-us")?
            }
            _ => {}
        }
    }

    if config.target_cycle_us <= 0.0 {
        return Err("--target-cycle-us must be greater than zero".to_string());
    }
    if config.duration_s <= 0.0 {
        return Err("--duration-s must be greater than zero".to_string());
    }
    if config.calibration_reads == 0 {
        return Err("--calibration-reads must be greater than zero".to_string());
    }

    Ok(config)
}

#[derive(Debug, Clone)]
pub enum LogicState {
    Finished(Result<(), String>),
}

fn get_time() -> i64 {
    let mut now = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        clock_gettime(CLOCK_MONOTONIC, &mut now);
    }

    now.tv_sec * 1_000_000_000 + now.tv_nsec
}

fn write_results(
    filename: &str,
    target_cycle_us: f64,
    x_trans: Vec<f64>,
    x_recv: Vec<f64>,
    times_at_begin: Vec<i64>,
    times_after_op: Vec<i64>,
    times_after_send: Vec<i64>,
    times_after_receive: Vec<i64>,
) -> Result<(), String> {
    let sample_count = times_at_begin.len();
    let mut cycle_duration_us: Vec<f64> = Vec::with_capacity(sample_count);
    let mut jitter_us: Vec<f64> = Vec::with_capacity(sample_count);
    let mut sleep_after_receive_us: Vec<f64> = Vec::with_capacity(sample_count);

    cycle_duration_us.push(f64::NAN);
    jitter_us.push(f64::NAN);
    for index in 1..sample_count {
        let cycle_ns = times_at_begin[index] - times_at_begin[index - 1];
        let cycle_us = ns_to_us(cycle_ns);
        cycle_duration_us.push(cycle_us);
        jitter_us.push(cycle_us - target_cycle_us);
    }

    for index in 0..sample_count {
        if index + 1 < sample_count {
            let sleep_ns = times_at_begin[index + 1] - times_after_receive[index];
            sleep_after_receive_us.push(ns_to_us(sleep_ns));
        } else {
            sleep_after_receive_us.push(f64::NAN);
        }
    }

    let columns = vec![
        Column::new("x_trans".into(), x_trans),
        Column::new("x_recv".into(), x_recv),
        Column::new("time_at_begin_ns".into(), times_at_begin),
        Column::new("time_after_op_ns".into(), times_after_op),
        Column::new("time_after_send_ns".into(), times_after_send),
        Column::new("time_after_receive_ns".into(), times_after_receive),
        Column::new("cycle_duration_us".into(), cycle_duration_us),
        Column::new("jitter_us".into(), jitter_us),
        Column::new("sleep_after_receive_us".into(), sleep_after_receive_us),
    ];
    let mut df = DataFrame::new(columns).map_err(|err| err.to_string())?;
    let mut file_descriptor = File::create(filename).map_err(|err| err.to_string())?;

    CsvWriter::new(&mut file_descriptor)
        .include_header(true)
        .with_separator(b',')
        .finish(&mut df)
        .map_err(|err| err.to_string())
}

fn write_calibration_results(
    filename: &str,
    commanded_values: Vec<f64>,
    measured_values: Vec<f64>,
    absolute_error: Vec<f64>,
) -> Result<(), String> {
    let columns = vec![
        Column::new("commanded_value".into(), commanded_values),
        Column::new("measured_value".into(), measured_values),
        Column::new("absolute_error".into(), absolute_error),
    ];
    let mut df = DataFrame::new(columns).map_err(|err| err.to_string())?;
    let mut file_descriptor = File::create(filename).map_err(|err| err.to_string())?;

    CsvWriter::new(&mut file_descriptor)
        .include_header(true)
        .with_separator(b',')
        .finish(&mut df)
        .map_err(|err| err.to_string())
}

fn configure_daq(daq: &mut ComediDaq, config: &AppConfig) {
    daq.set_config(config.device_path.clone(), false, 0);
    daq.set_data_config(
        config.ai_range_index,
        config.ao_range_index,
        config.ai_aref,
        config.ao_aref,
    );

    let mut input_ports: HashSet<String> = HashSet::new();
    let mut output_ports: HashSet<String> = HashSet::new();
    input_ports.insert(config.input_port.clone());
    output_ports.insert(config.output_port.clone());
    daq.set_active_ports(&input_ports, &output_ports);
}

fn run_calibration(config: &AppConfig) -> Result<(), String> {
    let calibration_points: [f64; 9] = [-2.0, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0];
    let settle_duration = Duration::from_micros(config.calibration_settle_us);

    let mut daq = ComediDaq::new();
    configure_daq(&mut daq, config);
    daq.try_open()?;

    let mut commanded_values: Vec<f64> = Vec::with_capacity(calibration_points.len());
    let mut measured_values: Vec<f64> = Vec::with_capacity(calibration_points.len());
    let mut absolute_error: Vec<f64> = Vec::with_capacity(calibration_points.len());

    for commanded in calibration_points {
        daq.write(commanded);
        thread::sleep(settle_duration);
        let measured = daq.read_average(config.calibration_reads);
        commanded_values.push(commanded);
        measured_values.push(measured);
        absolute_error.push((measured - commanded).abs());
    }

    write_calibration_results(
        &config.calibration_csv,
        commanded_values.clone(),
        measured_values.clone(),
        absolute_error,
    )?;

    let sample_count = commanded_values.len() as f64;
    let mean_x = commanded_values.iter().sum::<f64>() / sample_count;
    let mean_y = measured_values.iter().sum::<f64>() / sample_count;
    let covariance = commanded_values
        .iter()
        .zip(&measured_values)
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>()
        / sample_count;
    let variance_x = commanded_values
        .iter()
        .map(|x| (x - mean_x).powi(2))
        .sum::<f64>()
        / sample_count;
    let slope = covariance / variance_x;
    let intercept = mean_y - slope * mean_x;

    println!("Calibration results written to {}", config.calibration_csv);
    println!("Estimated transfer function: measured ~= {slope:.6} * commanded + {intercept:.6}");

    Ok(())
}

fn run_receiver_thread(config: AppConfig, logic_state_tx: Sender<LogicState>) -> Result<(), String> {
    let time_init = get_time();

    let time_increment = 0.0015;
    let sample_period = Duration::from_secs_f64(config.target_cycle_us / 1_000_000.0);
    let goal = (config.duration_s / sample_period.as_secs_f64()) as usize;

    let x = -1.3;
    let y = 1.0;
    let z = 1.0;

    let e = 3.0;
    let mu = 0.0021;
    let s = 4.0;
    let vh = 1.0;

    let model_derivatives = ModelDerivativeVariables::new(x, y, z);
    let temporal_variables = ModelTemporalVariables::new(e, mu, s, vh);

    let mut times_at_begin: Vec<i64> = Vec::with_capacity(goal);
    let mut times_after_op: Vec<i64> = Vec::with_capacity(goal);
    let mut times_after_send: Vec<i64> = Vec::with_capacity(goal);
    let mut times_after_receive: Vec<i64> = Vec::with_capacity(goal);
    let mut x_trans: Vec<f64> = Vec::with_capacity(goal);
    let mut x_recv: Vec<f64> = Vec::with_capacity(goal);

    let mut hr_model =
        HindmarshRoseRungeKutta::new(model_derivatives, temporal_variables, time_increment);

    let mut daq = ComediDaq::new();
    configure_daq(&mut daq, &config);
    if let Err(err) = daq.try_open() {
        eprintln!("DAQ open failed: {err}. Continuing without COMEDI I/O.");
    }

    let mut next_activation = ActiveRtBackend::init_sleep(sample_period);
    for _ in 0..goal {
        let time_at_begin = get_time();

        hr_model.calculate_hindmarsh_rose();
        let (x_sent, _, _) = hr_model.get_model_info();

        let time_after_op = get_time();

        daq.write(x_sent);

        let time_after_send = get_time();

        let x_read = daq.read();

        // if daq.is_open() && (x_sent - x_read).abs() > f64::EPSILON {
        //    println!("Incongruence on daq {x_sent} != {x_read}");
        //}
        // else
        // {
        //     println!("Received {x_sent} == {x_read}");
        // }

        let time_after_receive = get_time();

        x_trans.push(x_sent);
        x_recv.push(x_read);
        times_at_begin.push(time_at_begin);
        times_after_op.push(time_after_op);
        times_after_send.push(time_after_send);
        times_after_receive.push(time_after_receive);

        ActiveRtBackend::sleep(sample_period, &mut next_activation);
    }
    let time_end = times_after_receive.last().copied();

    write_results(
        &config.output_csv,
        config.target_cycle_us,
        x_trans,
        x_recv,
        times_at_begin,
        times_after_op,
        times_after_send,
        times_after_receive,
    )?;

    if let Some(time_end) = time_end {
        println!("Started at {time_init} ended at {time_end}");
    } else {
        return Err("No samples were captured".to_string());
    }

    let _ = logic_state_tx.send(LogicState::Finished(Ok(())));
    Ok(())
}

fn main() {
    let config = parse_args().unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    if config.mode == RunMode::Calibrate {
        if let Err(err) = run_calibration(&config) {
            eprintln!("{err}");
            std::process::exit(1);
        }
        return;
    }

    let (logic_state_tx, logic_state_rx) = std::sync::mpsc::channel::<LogicState>();

    let handle = rt_thread::RuntimeThread::spawn(move || {
        let result = run_receiver_thread(config, logic_state_tx.clone());
        if let Err(err) = result {
            let _ = logic_state_tx.send(LogicState::Finished(Err(err)));
        }
    })
    .unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    match logic_state_rx.recv() {
        Ok(LogicState::Finished(Ok(()))) => {}
        Ok(LogicState::Finished(Err(err))) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Worker thread exited unexpectedly: {err}");
            std::process::exit(1);
        }
    }

    if handle.join().is_err() {
        eprintln!("Worker thread panicked");
        std::process::exit(1);
    }
}
