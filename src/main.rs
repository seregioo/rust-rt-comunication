mod comedi;
mod hindmarsh_rose;
mod rt_thread;

use libc::{CLOCK_MONOTONIC, clock_gettime, timespec};
use polars_core::prelude::*;
use polars_io::prelude::*;
use std::{collections::HashSet, fs::File, sync::mpsc::Sender, time::Duration};

use crate::comedi::comedi_driver::ComediDaq;
use crate::hindmarsh_rose::{
    HindmarshRoseModel, HindmarshRoseRungeKutta, ModelDerivativeVariables, ModelTemporalVariables,
};
use crate::rt_thread::ActiveRtBackend;

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
    x_trans: Vec<f64>,
    x_recv: Vec<f64>,
    times_at_begin: Vec<i64>,
    times_after_op: Vec<i64>,
    times_after_send: Vec<i64>,
    times_after_receive: Vec<i64>,
) -> Result<(), String> {
    let columns = vec![
        Column::new("x_trans".into(), x_trans),
        Column::new("x_recv".into(), x_recv),
        Column::new("time_at_begin_ns".into(), times_at_begin),
        Column::new("time_after_op_ns".into(), times_after_op),
        Column::new("time_after_send_ns".into(), times_after_send),
        Column::new("time_after_receive_ns".into(), times_after_receive),
    ];
    let mut df = DataFrame::new(columns).map_err(|err| err.to_string())?;
    let mut file_descriptor = File::create(filename).map_err(|err| err.to_string())?;

    CsvWriter::new(&mut file_descriptor)
        .include_header(true)
        .with_separator(b',')
        .finish(&mut df)
        .map_err(|err| err.to_string())
}

fn run_receiver_thread(logic_state_tx: Sender<LogicState>) -> Result<(), String> {
    let time_init = get_time();

    let duration_s = 60.0 * 10.0;
    let time_increment = 0.0015;
    let frequency_hz = 10000.0;
    let sample_period = Duration::from_secs_f64(1.0 / frequency_hz);
    let goal = (frequency_hz * duration_s) as usize;

    let filename = "rust-test.csv";

    let x = -1.3;
    let y = 1.0;
    let z = 1.0;

    let e = 3.0;
    let mu = 0.0021;
    let s = 4.0;
    let vh = 1.0;

    let model_derivatives = ModelDerivativeVariables::new(x, y, z);
    let temporal_variables = ModelTemporalVariables::new(e, mu, s, vh);

    let mut times_at_begin = Vec::new();
    let mut times_after_op = Vec::new();
    let mut times_after_send = Vec::new();
    let mut times_after_receive = Vec::new();
    let mut x_trans = Vec::new();
    let mut x_recv = Vec::new();

    let mut hr_model =
        HindmarshRoseRungeKutta::new(model_derivatives, temporal_variables, time_increment);

    let mut daq = ComediDaq::new();
    println!("AO chanels:");
    println!("AI chanels:");
    let mut input_ports = HashSet::new();
    let mut output_ports = HashSet::new();

    input_ports.insert("a7".to_string());
    output_ports.insert("i0".to_string());

    daq.set_active_ports(&input_ports, &output_ports);
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
        filename,
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
    let (logic_state_tx, logic_state_rx) = std::sync::mpsc::channel::<LogicState>();

    let handle = rt_thread::RuntimeThread::spawn(move || {
        let result = run_receiver_thread(logic_state_tx.clone());
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
