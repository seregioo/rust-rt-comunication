#[derive(Copy, Clone)]
pub struct ModelDerivativeVariables {
    x: f64,
    y: f64,
    z: f64,
}
impl ModelDerivativeVariables {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn get_vars(&self) -> (f64, f64, f64) {
        (self.x, self.y, self.z)
    }
}

#[derive(Copy, Clone)]
pub struct ModelTemporalVariables {
    e: f64,
    mu: f64,
    s: f64,
    vh: f64,
}
impl ModelTemporalVariables {
    pub fn new(e: f64, mu: f64, s: f64, vh: f64) -> Self {
        Self { e, mu, s, vh }
    }

    pub fn get_vars(&self) -> (f64, f64, f64, f64) {
        (self.e, self.mu, self.s, self.vh)
    }
}

pub struct HindmarshRoseEuler {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    e: f64,
    mu: f64,
    s: f64,
    vh: f64,
    time_increment: f64,
    i_syn: f64,
}

pub struct HindmarshRoseRungeKutta {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    e: f64,
    mu: f64,
    s: f64,
    vh: f64,
    time_increment: f64,
    i_syn: f64,
}

pub trait HindmarshRoseModel {
    fn calculate_hindmarsh_rose(&mut self);
    fn update_e(&mut self, new_e: f64);
    fn update_i_syn(&mut self, new_i_syn: f64);
    fn get_e(&self) -> f64;
    fn get_model_info(&self) -> (f64, f64, f64);
}

impl HindmarshRoseEuler {
    pub fn new(
        model_derivatives: ModelDerivativeVariables,
        temporal_variables: ModelTemporalVariables,
        time_increment: f64,
    ) -> Self {
        let (x, y, z) = model_derivatives.get_vars();
        let (e, mu, s, vh) = temporal_variables.get_vars();

        Self {
            x,
            y,
            z,
            time_increment,
            e,
            mu,
            s,
            vh,
            i_syn: 0.0,
        }
    }
}

impl HindmarshRoseRungeKutta {
    pub fn new(
        model_derivatives: ModelDerivativeVariables,
        temporal_variables: ModelTemporalVariables,
        time_increment: f64,
    ) -> Self {
        let (x, y, z) = model_derivatives.get_vars();
        let (e, mu, s, vh) = temporal_variables.get_vars();

        Self {
            x,
            y,
            z,
            time_increment,
            e,
            mu,
            s,
            vh,
            i_syn: 0.0,
        }
    }
}

impl HindmarshRoseModel for HindmarshRoseEuler {
    fn calculate_hindmarsh_rose(&mut self) {
        let xi = self.x
            + self.time_increment
                * (self.y + 3.0 * self.x * self.x - self.x * self.x * self.x - self.z + self.e
                    - self.i_syn);
        let yi = self.y + self.time_increment * (1.0 - 5.0 * self.x * self.x - self.y);
        let zi =
            self.z + self.time_increment * self.mu * (-self.vh * self.z + self.s * (self.x + 1.6));

        self.x = xi;
        self.y = yi;
        self.z = zi;
    }
    fn update_e(&mut self, new_e: f64) {
        self.e = new_e;
    }
    fn update_i_syn(&mut self, new_i_syn: f64) {
        self.i_syn = new_i_syn
    }
    fn get_model_info(&self) -> (f64, f64, f64) {
        (self.x, self.y, self.z)
    }
    fn get_e(&self) -> f64 {
        self.e
    }
}

impl HindmarshRoseModel for HindmarshRoseRungeKutta {
    fn calculate_hindmarsh_rose(&mut self) {
        let dt = self.time_increment;

        // --- k1 ---
        let dx1 = self.y + 3.0 * self.x * self.x - self.x * self.x * self.x - self.vh * self.z
            + self.e
            - self.i_syn;
        let dy1 = 1.0 - 5.0 * self.x * self.x - self.y;
        let dz1 = self.mu * (-self.vh * self.z + self.s * (self.x + 1.6));

        // --- k2 ---
        let x2 = self.x + 0.5 * dt * dx1;
        let y2 = self.y + 0.5 * dt * dy1;
        let z2 = self.z + 0.5 * dt * dz1;
        let dx2 = y2 + 3.0 * x2 * x2 - x2 * x2 * x2 - z2 + self.e - self.i_syn;
        let dy2 = 1.0 - 5.0 * x2 * x2 - y2;
        let dz2 = self.mu * (-z2 + self.s * (x2 + 1.6));

        // --- k3 ---
        let x3 = self.x + 0.5 * dt * dx2;
        let y3 = self.y + 0.5 * dt * dy2;
        let z3 = self.z + 0.5 * dt * dz2;
        let dx3 = y3 + 3.0 * x3 * x3 - x3 * x3 * x3 - z3 + self.e - self.i_syn;
        let dy3 = 1.0 - 5.0 * x3 * x3 - y3;
        let dz3 = self.mu * (-z3 + self.s * (x3 + 1.6));

        // --- k4 ---
        let x4 = self.x + dt * dx3;
        let y4 = self.y + dt * dy3;
        let z4 = self.z + dt * dz3;
        let dx4 = y4 + 3.0 * x4 * x4 - x4 * x4 * x4 - z4 + self.e - self.i_syn;
        let dy4 = 1.0 - 5.0 * x4 * x4 - y4;
        let dz4 = self.mu * (-z4 + self.s * (x4 + 1.6));

        // --- Update (weighted average of slopes) ---
        self.x += (dt / 6.0) * (dx1 + 2.0 * dx2 + 2.0 * dx3 + dx4);
        self.y += (dt / 6.0) * (dy1 + 2.0 * dy2 + 2.0 * dy3 + dy4);
        self.z += (dt / 6.0) * (dz1 + 2.0 * dz2 + 2.0 * dz3 + dz4);
    }
    fn update_e(&mut self, new_e: f64) {
        self.e = new_e;
    }
    fn update_i_syn(&mut self, new_i_syn: f64) {
        self.i_syn = new_i_syn
    }
    fn get_model_info(&self) -> (f64, f64, f64) {
        (self.x, self.y, self.z)
    }
    fn get_e(&self) -> f64 {
        self.e
    }
}
