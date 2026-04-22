mod comedi;
mod hindmarsh_rose;
mod rt_thread;

use libc::{
    CLOCK_MONOTONIC, SCHED_FIFO, SYS_gettid, TIMER_ABSTIME, clock_gettime, clock_nanosleep,
    sched_param, sched_setscheduler, syscall, timespec,
};
use polars_core::prelude::*;
use polars_io::prelude::*;
use std::{
    collections::HashSet,
    fs::File,
    sync::mpsc::{Receiver, Sender, TryRecvError},
};

use crate::comedi::comedi_driver::{ComediDaq, from_phys, open, to_phys, write};
use crate::hindmarsh_rose::{
    HindmarshRoseModel, HindmarshRoseRungeKutta, ModelDerivativeVariables, ModelTemporalVariables,
};

#[derive(Debug, Clone)]
pub enum LogicState {
    End,
}

fn get_time() -> i64 {
    let mut now = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        clock_gettime(CLOCK_MONOTONIC, &mut now);
    }
    return now.tv_nsec;
}

fn run_receiver_thread(logic_state_tx: Sender<LogicState>) {
    let time_init = get_time();
    // loop {
    //     match logic_rx.try_recv() {
    //         Ok(message) => match message {
    //             LogicMessage::Value(value) => {}
    //         },
    //
    //         Err(TryRecvError::Empty) => break,
    //         Err(TryRecvError::Disconnected) => break,
    //     }
    // }
    let time_init = get_time();

    let duration_s = 60.0 * 10.0;
    let time_increment = 0.0015;
    let mut time_counter = 0.0;

    let frequency_hz = 10000.0;
    let goal = frequency_hz * duration_s;

    let filename: String = "rust-test.csv".to_string();

    let downsample_rate = 100;

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

    let mut hr_model =
        HindmarshRoseRungeKutta::new(model_derivatives, temporal_variables, time_increment);

    let mut daq = ComediDaq::new();

    let mut input_ports = HashSet::new();
    let mut output_ports = HashSet::new();

    input_ports.insert("a0".to_string());
    output_ports.insert("i7".to_string());

    daq.set_active_ports(&input_ports, &output_ports);

    while time_counter < goal {
        let time_at_begin = get_time();

        hr_model.calculate_hindmarsh_rose();
        let (x_sent, _, _) = hr_model.get_model_info();
        time_counter += time_increment;

        let time_after_op = get_time();

        daq.write(x_sent);

        let time_after_send = get_time();

        let x_read = daq.read();

        if x_sent != x_read {
            println!("Incongruence on daq {x_sent} != {x_read}");
        }

        let time_after_recieve = get_time();

        times_at_begin.push(time_at_begin);
        times_after_op.push(time_after_op);
        times_after_send.push(time_after_send);
        times_after_receive.push(time_after_recieve);
    }
    let mut columns = Vec::new();

    let column = Column::new("time_at_begin".into(), times_at_begin);
    columns.push(column);
    let df: PolarsResult<DataFrame> = DataFrame::new(columns);

    let mut df = df.unwrap();

    let mut file_descriptor = File::create(filename).unwrap();

    let _ = CsvWriter::new(&mut file_descriptor)
        .include_header(true)
        .with_separator(b',')
        .finish(&mut df);

    if let Some(time_end) = times_after_receive.last() {
        println!("Started at {time_init} ended at {time_end}");
    } else {
        println!("Program failed")
    }

    let _ = logic_state_tx.send(LogicState::End);
}
fn main() {
    let (logic_state_tx, logic_state_rx) = std::sync::mpsc::channel::<LogicState>();

    rt_thread::RuntimeThread::spawn(move || {
        let _ = run_receiver_thread(logic_state_tx);
    })
    .unwrap();

    loop {
        match logic_state_rx.try_recv() {
            Ok(_) => break,
            Err(_) => break,
        }
    }
}
