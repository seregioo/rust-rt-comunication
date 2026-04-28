pub mod comedi_driver {
    pub trait DeviceDriver {
        fn open(&mut self) -> Result<(), String>;
        fn close(&mut self);
    }

    pub struct ComediDaq {
        input_port_names: Vec<String>,
        output_port_names: Vec<String>,

        device_path: String,
        pub ai_channels: Vec<(u32, u32)>,
        pub ao_channels: Vec<(u32, u32)>,

        input_values: HashMap<String, f64>,
        output_values: HashMap<String, f64>,

        is_open: bool,
        last_scan_devices: bool,
        last_scan_nonce: u64,
        active_inputs: Vec<bool>,
        active_outputs: Vec<bool>,
        ao_port_names: Vec<String>,
        ai_port_names: Vec<String>,
        ao_calibration: Vec<Option<(comedilib::comedi_range, comedilib::LsamplT)>>,
        ai_calibration: Vec<Option<(comedilib::comedi_range, comedilib::LsamplT)>>,
        no_device_detected: bool,
        dev: Option<std::ptr::NonNull<comedilib::comedi_t>>,
    }

    impl ComediDaq {
        fn normalize_device_path(path: &str) -> &str {
            if let Some(idx) = path.find("_subd") {
                &path[..idx]
            } else {
                path
            }
        }

        pub fn new() -> Self {
            let mut plugin = Self {
                input_port_names: Vec::new(),
                output_port_names: Vec::new(),
                device_path: "/dev/comedi0".to_string(),
                ai_channels: Vec::new(),
                ao_channels: Vec::new(),
                input_values: HashMap::new(),
                output_values: HashMap::new(),
                is_open: false,
                last_scan_devices: false,
                last_scan_nonce: 0,
                active_inputs: Vec::new(),
                active_outputs: Vec::new(),
                ao_port_names: Vec::new(),
                ai_port_names: Vec::new(),
                ao_calibration: Vec::new(),
                ai_calibration: Vec::new(),
                no_device_detected: false,
                dev: None,
            };

            plugin.auto_configure();
            plugin
        }

        pub fn set_config(&mut self, device_path: String, scan_devices: bool, scan_nonce: u64) {
            let changed = self.device_path != device_path;
            if changed {
                self.device_path = device_path;
            }
            if changed
                || (scan_devices && !self.last_scan_devices)
                || scan_nonce != self.last_scan_nonce
            {
                self.auto_configure();
            }
            self.last_scan_devices = scan_devices;
            self.last_scan_nonce = scan_nonce;
        }

        pub fn set_input(&mut self, port_name: &str, value: f64) {
            self.input_values.insert(port_name.to_string(), value);
        }

        pub fn get_output(&self, port_name: &str) -> f64 {
            self.output_values.get(port_name).copied().unwrap_or(0.0)
        }

        pub fn is_open(&self) -> bool {
            self.is_open
        }

        pub fn set_active_ports(
            &mut self,
            input_ports: &std::collections::HashSet<String>,
            output_ports: &std::collections::HashSet<String>,
        ) {
            for x in &self.input_port_names
            {
                println!("{x}");
            }
            for x in &self.output_port_names
            {
                println!("{x}");
            }
            if self.active_inputs.len() != self.input_port_names.len() {
                self.active_inputs
                    .resize(self.input_port_names.len(), false);
            }
            if self.active_outputs.len() != self.output_port_names.len() {
                self.active_outputs
                    .resize(self.output_port_names.len(), false);
            }
            for (idx, name) in self.input_port_names.iter().enumerate() {
                self.active_inputs[idx] = input_ports.contains(name);
            }
            for (idx, name) in self.output_port_names.iter().enumerate() {
                self.active_outputs[idx] = output_ports.contains(name);
            }
        }

        fn auto_configure(&mut self) {
            let device_path = Self::normalize_device_path(&self.device_path);
            let Ok(dev) = (unsafe { comedilib::open(device_path) }) else {
                self.no_device_detected = true;
                self.ai_channels.clear();
                self.ao_channels.clear();
                self.input_port_names.clear();
                self.output_port_names.clear();
                return;
            };

            let mut ai = Vec::new();
            let mut ao = Vec::new();
            let n = unsafe { comedilib::get_n_subdevices(dev).unwrap_or(0) };
            for sd in 0..n {
                match unsafe { comedilib::get_subdevice_type(dev, sd) } {
                    Ok(t) if t == comedilib::SUBD_AI => {
                        let ch = unsafe { comedilib::get_n_channels(dev, sd).unwrap_or(0) };
                        for c in 0..ch {
                            ai.push((sd, c));
                        }
                    }
                    Ok(t) if t == comedilib::SUBD_AO => {
                        let ch = unsafe { comedilib::get_n_channels(dev, sd).unwrap_or(0) };
                        for c in 0..ch {
                            ao.push((sd, c));
                        }
                    }
                    _ => {}
                }
            }

            self.no_device_detected = false;
            self.ai_channels = ai;
            self.ao_channels = ao;
            self.input_port_names = self
                .ai_channels
                .iter()
                .map(|(_, ch)| format!("a{ch}"))
                .collect();
            self.output_port_names = self
                .ao_channels
                .iter()
                .map(|(_, ch)| format!("i{ch}"))
                .collect();
            self.active_inputs.resize(self.input_port_names.len(), false);
            self.active_outputs.resize(self.output_port_names.len(), false);
            unsafe { comedilib::close(dev) };
        }

        fn rebuild_calibration_cache(&mut self) -> Result<(), String> {
            let Some(dev) = self.dev.as_ref() else {
                if self.no_device_detected {
                    if let Some(value) = self.output_values.get_mut("no device detected") {
                        *value = 1.0;
                    } else {
                        self.output_values
                            .insert("no device detected".to_string(), 1.0);
                    }
                }
                return Ok(());
            };
            let dev = dev.as_ptr();
            self.ao_calibration.clear();
            self.ao_calibration.reserve(self.ao_channels.len());
            for (sd, ch) in &self.ao_channels {
                let range = unsafe { comedilib::get_range(dev, *sd, *ch) }.unwrap();
                let max = unsafe { comedilib::get_maxdata(dev, *sd, *ch) }.unwrap();
                self.ao_calibration.push(Some((range, max)));
            }
            self.ai_calibration.clear();
            self.ai_calibration.reserve(self.ai_channels.len());
            for (sd, ch) in &self.ai_channels {
                let range = unsafe { comedilib::get_range(dev, *sd, *ch) }.unwrap();
                let max = unsafe { comedilib::get_maxdata(dev, *sd, *ch) }.unwrap();
                self.ai_calibration.push(Some((range, max)));
            }
            Ok(())
        }

        pub fn try_open(&mut self) -> Result<(), String> {
            if self.is_open || self.no_device_detected {
                return Ok(());
            }

            <Self as DeviceDriver>::open(self)
        }

        pub fn read(&mut self) -> f64 {
            let Some(dev) = self.dev.as_ref() else {
                return 0.0;
            };
            let dev = dev.as_ptr();

            for (idx, (sd, ch)) in self.ai_channels.iter().enumerate() {
                if self.active_inputs.get(idx).copied() == Some(false) {
                    continue;
                }

                let raw = unsafe { comedilib::read(dev, *sd, *ch) }.unwrap();
                let Some((range, max)) = self.ai_calibration.get(idx).and_then(|v| *v) else {
                    continue;
                };
                let phys = unsafe { comedilib::to_phys(raw, &range, max) };

                return phys;
            }
            0.0
        }

        pub fn write(&mut self, value: f64) {
            let Some(dev) = self.dev.as_ref() else {
                return;
            };
            let dev = dev.as_ptr();
            for (idx, (sd, ch)) in self.ao_channels.iter().enumerate() {
                if self.active_outputs.get(idx).copied() == Some(false) {
                    continue;
                }

                let Some((range, max)) = self.ao_calibration.get(idx).and_then(|v| *v) else {
                    continue;
                };
                let raw = unsafe { comedilib::from_phys(value, &range, max) };
                let _ = unsafe { comedilib::write(dev, *sd, *ch, raw) };
            }
        }
    }

    impl DeviceDriver for ComediDaq {
        fn open(&mut self) -> Result<(), String> {
            let device_path = Self::normalize_device_path(&self.device_path);
            let dev = unsafe { comedilib::open(device_path) }?;
            self.dev = std::ptr::NonNull::new(dev);
            self.rebuild_calibration_cache()?;
            self.is_open = true;
            Ok(())
        }

        fn close(&mut self) {
            if let Some(dev) = self.dev.take() {
                unsafe { comedilib::close(dev.as_ptr()) };
            }
            self.ao_calibration.clear();
            self.ai_calibration.clear();
            self.is_open = false;
        }
    }
    use std::{
        collections::HashMap,
        ffi::{CStr, CString, c_uint},
    };

    use crate::comedi::comedilib::{
        self, LsamplT, comedi_data_read, comedi_data_write, comedi_from_phys, comedi_open,
        comedi_range, comedi_t, comedi_to_phys,
    };

    fn last_error() -> String {
        unsafe {
            let err = comedilib::comedi_errno();
            let msg = comedilib::comedi_strerror(err);
            if msg.is_null() {
                format!("comedi error {err}")
            } else {
                CStr::from_ptr(msg).to_string_lossy().to_string()
            }
        }
    }

    pub fn open(path: &str) -> Result<*mut comedi_t, String> {
        let cpath = CString::new(path).map_err(|_| "invalid device path".to_string())?;
        unsafe {
            let dev = comedi_open(cpath.as_ptr());
            if dev.is_null() {
                Err(last_error())
            } else {
                Ok(dev)
            }
        }
    }

    pub fn read(dev: *mut comedi_t, subd: u32, chan: u32) -> Result<LsamplT, String> {
        let mut data: LsamplT = 0;
        unsafe {
            let res = comedi_data_read(dev, subd as c_uint, chan as c_uint, 0, 0, &mut data);
            if res < 0 { Err(last_error()) } else { Ok(data) }
        }
    }

    pub fn write(dev: *mut comedi_t, subd: u32, chan: u32, data: LsamplT) -> Result<(), String> {
        unsafe {
            let res = comedi_data_write(dev, subd as c_uint, chan as c_uint, 0, 0, data);
            if res < 0 { Err(last_error()) } else { Ok(()) }
        }
    }
    pub fn from_phys(data: f64, range: &comedi_range, maxdata: LsamplT) -> LsamplT {
        unsafe { comedi_from_phys(data, range as *const comedi_range, maxdata) }
    }

    pub fn to_phys(data: LsamplT, range: &comedi_range, maxdata: LsamplT) -> f64 {
        unsafe { comedi_to_phys(data, range as *const comedi_range, maxdata) as f64 }
    }
}
mod comedilib {
    use libc::{c_char, c_double, c_int, c_uint};
    use std::ffi::{CStr, CString};

    #[repr(C)]
    pub struct comedi_t {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct comedi_range {
        pub min: c_double,
        pub max: c_double,
        pub unit: c_uint,
    }

    pub type LsamplT = c_uint;

    pub const SUBD_AI: c_int = 1;
    pub const SUBD_AO: c_int = 2;

    #[link(name = "comedi")]
    unsafe extern "C" {
        pub fn comedi_open(fn_ptr: *const c_char) -> *mut comedi_t;
        pub fn comedi_close(dev: *mut comedi_t) -> c_int;
        pub fn comedi_errno() -> c_int;
        pub fn comedi_strerror(errnum: c_int) -> *const c_char;

        pub fn comedi_get_n_subdevices(dev: *mut comedi_t) -> c_int;
        pub fn comedi_get_subdevice_type(dev: *mut comedi_t, subdevice: c_uint) -> c_int;
        pub fn comedi_get_n_channels(dev: *mut comedi_t, subdevice: c_uint) -> c_int;

        pub fn comedi_get_range(
            dev: *mut comedi_t,
            subdevice: c_uint,
            chan: c_uint,
            range: c_uint,
        ) -> *mut comedi_range;
        pub fn comedi_get_maxdata(dev: *mut comedi_t, subdevice: c_uint, chan: c_uint) -> LsamplT;

        pub fn comedi_to_phys(
            data: LsamplT,
            rng: *const comedi_range,
            maxdata: LsamplT,
        ) -> c_double;
        pub fn comedi_from_phys(
            data: c_double,
            rng: *const comedi_range,
            maxdata: LsamplT,
        ) -> LsamplT;

        pub fn comedi_data_read(
            dev: *mut comedi_t,
            subd: c_uint,
            chan: c_uint,
            range: c_uint,
            aref: c_uint,
            data: *mut LsamplT,
        ) -> c_int;
        pub fn comedi_data_write(
            dev: *mut comedi_t,
            subd: c_uint,
            chan: c_uint,
            range: c_uint,
            aref: c_uint,
            data: LsamplT,
        ) -> c_int;
    }

    fn last_error() -> String {
        unsafe {
            let err = comedi_errno();
            let msg = comedi_strerror(err);
            if msg.is_null() {
                format!("comedi error {err}")
            } else {
                CStr::from_ptr(msg).to_string_lossy().to_string()
            }
        }
    }

    pub unsafe fn open(path: &str) -> Result<*mut comedi_t, String> {
        let cpath = CString::new(path).map_err(|_| "invalid device path".to_string())?;
        let dev = unsafe { comedi_open(cpath.as_ptr()) };
        if dev.is_null() {
            Err(last_error())
        } else {
            Ok(dev)
        }
    }

    pub unsafe fn close(dev: *mut comedi_t) {
        let _ = unsafe { comedi_close(dev) };
    }

    pub unsafe fn get_n_subdevices(dev: *mut comedi_t) -> Result<u32, String> {
        let n = unsafe { comedi_get_n_subdevices(dev) };
        if n < 0 {
            Err(last_error())
        } else {
            Ok(n as u32)
        }
    }

    pub unsafe fn get_subdevice_type(dev: *mut comedi_t, subd: u32) -> Result<i32, String> {
        let t = unsafe { comedi_get_subdevice_type(dev, subd as c_uint) };
        if t < 0 { Err(last_error()) } else { Ok(t) }
    }

    pub unsafe fn get_n_channels(dev: *mut comedi_t, subd: u32) -> Result<u32, String> {
        let n = unsafe { comedi_get_n_channels(dev, subd as c_uint) };
        if n < 0 {
            Err(last_error())
        } else {
            Ok(n as u32)
        }
    }

    pub unsafe fn get_range(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
    ) -> Result<comedi_range, String> {
        let ptr = unsafe { comedi_get_range(dev, subd as c_uint, chan as c_uint, 0) };
        if ptr.is_null() {
            Err(last_error())
        } else {
            Ok(unsafe { *ptr })
        }
    }

    pub unsafe fn get_maxdata(dev: *mut comedi_t, subd: u32, chan: u32) -> Result<LsamplT, String> {
        let val = unsafe { comedi_get_maxdata(dev, subd as c_uint, chan as c_uint) };
        if val == 0 {
            let err = unsafe { comedi_errno() };
            if err != 0 { Err(last_error()) } else { Ok(val) }
        } else {
            Ok(val)
        }
    }

    pub unsafe fn to_phys(data: LsamplT, range: &comedi_range, maxdata: LsamplT) -> f64 {
        unsafe { comedi_to_phys(data, range as *const comedi_range, maxdata) as f64 }
    }

    pub unsafe fn from_phys(data: f64, range: &comedi_range, maxdata: LsamplT) -> LsamplT {
        unsafe { comedi_from_phys(data, range as *const comedi_range, maxdata) }
    }

    pub unsafe fn read(dev: *mut comedi_t, subd: u32, chan: u32) -> Result<LsamplT, String> {
        let mut data: LsamplT = 0;
        let res = unsafe { comedi_data_read(dev, subd as c_uint, chan as c_uint, 0, 0, &mut data) };
        if res < 0 { Err(last_error()) } else { Ok(data) }
    }

    pub unsafe fn write(
        dev: *mut comedi_t,
        subd: u32,
        chan: u32,
        data: LsamplT,
    ) -> Result<(), String> {
        let res = unsafe { comedi_data_write(dev, subd as c_uint, chan as c_uint, 0, 0, data) };
        if res < 0 { Err(last_error()) } else { Ok(()) }
    }
}
