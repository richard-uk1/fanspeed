use std::{
    fs, 
    thread::{spawn, sleep, JoinHandle}, 
    sync::{
        atomic::{AtomicBool, Ordering}, 
        mpsc::{self, SyncSender, Receiver, TryRecvError}, 
    },
    time::Duration,
};
use anyhow::{Result, Error};
use gpiod::{Chip, Lines, Output, Options, Active, Drive};

const RAMP_START: f64 = 40.;
const RAMP_END: f64 = 60.;
const FULL_PULSE: Duration = Duration::from_millis(1);
const SAMPLE_RATE: Duration = Duration::from_millis(500);

static TERMINATION_SIGNAL: AtomicBool = AtomicBool::new(false);

fn main() -> Result<()> {
    ctrlc::set_handler(move || {
        TERMINATION_SIGNAL.store(true, Ordering::Relaxed);
    }).unwrap();
    let fan = Fan::init()?;
    while !TERMINATION_SIGNAL.load(Ordering::Relaxed) {
        let temp = get_temp()?;
        fan.send_temp(temp)?;
        sleep(SAMPLE_RATE);
    }
    fan.shutdown()?;
    Ok(())
}

/// Get temperature in degrees Celsius
fn get_temp() -> Result<f64> {
    Ok(fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")?.trim().parse::<f64>()? / 1000.)
}

struct Fan {
    sender: SyncSender<f64>,
    handle: JoinHandle<Result<()>>
}

impl Fan {
    fn init() -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let handle = spawn(move || {
            let inner = FanInner::new(rx)?;
            inner.run()
        });
        Ok(Self {
            sender: tx,
            handle
        })
    }

    fn send_temp(&self, temp: f64) -> Result<()> {
        Ok(self.sender.send(temp)?)
    }


    fn shutdown(self) -> Result<()> {
        drop(self.sender);
        self.handle.join().unwrap()
    }
}

struct FanInner {
    pwm: f64,
    #[allow(unused)]
    chip: Chip,
    lines: Lines<Output>,
    rx: Receiver<f64>
}

impl FanInner {
    fn new(rx: Receiver<f64>) -> Result<Self> {
        let chip = Chip::new(0)?;
        let opts = Options::output([14])
            .active(Active::High)
            .drive(Drive::PushPull)
            .values([true])
            .consumer("fan-ctrl");
        let lines = chip.request_lines(opts)?;
        Ok(Self { pwm: 1., chip, lines, rx })
    }

    fn run(mut self) -> Result<()> {
        loop {
            match self.rx.try_recv() {
                Ok(temp) => self.set_temp(temp),
                Err(TryRecvError::Disconnected) => {
                    // leave fan on
                    self.set_on()?;
                    break Ok(());
                }
                Err(TryRecvError::Empty) => (),
            }
            self.pulse()?;
        }
    }
    
    fn set_temp(&mut self, temp: f64) {
        self.pwm = ((temp - RAMP_START) / (RAMP_END - RAMP_START)).clamp(0., 1.);
        if !self.pwm.is_finite() {
            self.pwm = 1.;
        }
        println!("Temp: {temp}°C");
        println!("PWM: {}", self.pwm);
    }

    fn pulse(&self) -> Result<()> {
        if self.pwm >= 1. {
            self.set_on()?;
            sleep(FULL_PULSE);
        } else if self.pwm <= 0. {
            self.set_off()?;
            sleep(FULL_PULSE);
        } else {
            self.set_on()?;
            sleep(FULL_PULSE.mul_f64(self.pwm));
            self.set_off()?;
            sleep(FULL_PULSE.mul_f64(1. - self.pwm));
        }
        Ok(())
    }

    fn set_on(&self) -> Result<(), Error> {
        self.lines.set_values([true])?;
        Ok(())
    }

    fn set_off(&self) -> Result<(), Error> {
        self.lines.set_values([false])?;
        Ok(())
    }
}
