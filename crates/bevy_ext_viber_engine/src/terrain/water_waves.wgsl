// Dispersive iWave + river-advected interactive water surface compute solver.
// Operates over a 512x512 rolling 0.25 m/texel window (128 m x 128 m).

struct WaveImpulse {
    // xy = world XZ center, zw = world XZ velocity (m/s)
    pos_vel: vec4<f32>,
    // x = radius (m), y = signed energy/displacement (m),
    // z = foam injection, w = kind (0.0 impact crater, 1.0 moving Kelvin dipole)
    params: vec4<f32>,
};

struct WaveSimUniforms {
    // xy = world origin XZ (m), zw = (window_size_m, texel_m)
    origin_size: vec4<f32>,
    // x = shift_x (fine texels), y = shift_z (fine texels), z = dt (s), w = elapsed_time (s)
    shift_dt_time: vec4<f32>,
    // x = rain_intensity, y = wind_x, z = wind_z, w = impulse_count
    env_count: vec4<f32>,
    // x = calibrated restoring coeff g_eff, yzw = reserved
    physics: vec4<f32>,
    // Symmetric quadrant |dx|<=5, |dz|<=5 of 11x11 dispersive Hankel kernel (36 floats)
    kernel: array<vec4<f32>, 9>,
    impulses: array<WaveImpulse, 64>,
};

@group(0) @binding(0) var<uniform> sim: WaveSimUniforms;
@group(0) @binding(1) var<storage, read> state_in: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> state_out: array<vec4<f32>>;
@group(0) @binding(3) var domain_tex: texture_2d<f32>;
@group(0) @binding(4) var domain_sampler: sampler;
@group(0) @binding(5) var wave_out_tex: texture_storage_2d<rgba16float, write>;

const GRID_U: u32 = 512u;
const GRID_I: i32 = 512;
const KERNEL_R: i32 = 5;
const TILE_DIM: u32 = 18u; // 8 + 2 * 5

var<workgroup> tile_h: array<f32, 324>;

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn sanitize(v: f32) -> f32 {
    // NaN fails self-equality; clamp finite values safely.
    if !(v == v) {
        return 0.0;
    }
    return clamp(v, -0.45, 0.45);
}

fn load_state(ix: i32, iz: i32) -> vec4<f32> {
    if ix < 0 || ix >= GRID_I || iz < 0 || iz >= GRID_I {
        return vec4<f32>(0.0);
    }
    let s = state_in[u32(iz) * GRID_U + u32(ix)];
    return vec4<f32>(sanitize(s.x), sanitize(s.y), clamp(select(0.0, s.z, s.z == s.z), 0.0, 1.0), 0.0);
}

fn sample_state_bilinear(pos: vec2<f32>) -> vec4<f32> {
    let f = floor(pos);
    let w = pos - f;
    let i0 = vec2<i32>(i32(f.x), i32(f.y));
    let s00 = load_state(i0.x, i0.y);
    let s10 = load_state(i0.x + 1, i0.y);
    let s01 = load_state(i0.x, i0.y + 1);
    let s11 = load_state(i0.x + 1, i0.y + 1);
    return mix(mix(s00, s10, w.x), mix(s01, s11, w.x), w.y);
}

@compute @workgroup_size(8, 8, 1)
fn step_sim(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let shift = vec2<i32>(
        i32(round(sim.shift_dt_time.x)),
        i32(round(sim.shift_dt_time.y)),
    );
    let base = vec2<i32>(i32(wid.x) * 8 - KERNEL_R, i32(wid.y) * 8 - KERNEL_R);
    let tid = lid.y * 8u + lid.x;

    // Cooperatively load shifted height neighborhood into workgroup shared memory.
    for (var k = tid; k < 324u; k += 64u) {
        let lx = i32(k % TILE_DIM);
        let lz = i32(k / TILE_DIM);
        let rx = base.x + lx + shift.x;
        let rz = base.y + lz + shift.y;
        tile_h[k] = load_state(rx, rz).x;
    }
    workgroupBarrier();

    let gx = i32(gid.x);
    let gz = i32(gid.y);
    let idx = gid.y * GRID_U + gid.x;

    let texel_m = sim.origin_size.w;
    let world_xz = sim.origin_size.xy + (vec2<f32>(f32(gx), f32(gz)) + vec2<f32>(0.5)) * texel_m;
    let uv = (vec2<f32>(f32(gx), f32(gz)) + vec2<f32>(0.5)) / f32(GRID_U);
    let domain = textureSampleLevel(domain_tex, domain_sampler, uv, 0.0);

    let depth_m = max(domain.x, 0.0);
    let flow = domain.yz;
    let plunge = clamp(domain.w, 0.0, 1.0);
    let dt = clamp(sim.shift_dt_time.z, 0.001, 0.025);
    let time = sim.shift_dt_time.w;

    // Neighborhood depth probes (1.0 m grid spacing) to detect shorelines, gravel bars, and rock banks.
    let dom_step = 1.0 / 128.0;
    let d_xp = max(textureSampleLevel(domain_tex, domain_sampler, uv + vec2<f32>(dom_step, 0.0), 0.0).x, 0.0);
    let d_xm = max(textureSampleLevel(domain_tex, domain_sampler, uv - vec2<f32>(dom_step, 0.0), 0.0).x, 0.0);
    let d_zp = max(textureSampleLevel(domain_tex, domain_sampler, uv + vec2<f32>(0.0, dom_step), 0.0).x, 0.0);
    let d_zm = max(textureSampleLevel(domain_tex, domain_sampler, uv - vec2<f32>(0.0, dom_step), 0.0).x, 0.0);
    let min_nb_depth = min(min(d_xp, d_xm), min(d_zp, d_zm));
    let grad_depth = vec2<f32>(d_xp - d_xm, d_zp - d_zm) * 0.5;
    let shore_slope = length(grad_depth);
    let shore_prox = clamp(
        max(1.0 - smoothstep(0.02, 0.72, min_nb_depth), smoothstep(0.46, 0.06, depth_m)),
        0.0,
        1.0,
    );

    // Boundary-layer bank friction: current decelerates near shallow beds and riverbanks,
    // deflects along bank walls, and sheds tiny shoreline shear ripples.
    let bed_flow_drag = smoothstep(0.02, 0.48, depth_m);
    let bank_flow_drag = 0.24 + 0.76 * smoothstep(0.02, 0.78, min_nb_depth);
    let flow_raw_sq = dot(flow, flow);
    var flow_eff = flow * (bed_flow_drag * bank_flow_drag);
    if shore_slope > 1e-3 && flow_raw_sq > 1e-4 {
        let n_in = grad_depth / shore_slope;
        let t_bank = vec2<f32>(-n_in.y, n_in.x);
        let v_n = dot(flow_eff, n_in);
        let v_t = dot(flow_eff, t_bank);
        let deflected_n = select(v_n, v_n * (1.0 - 0.65 * shore_prox), v_n < 0.0);
        let eddy_wobble = sin(dot(world_xz, t_bank) * 2.1 - time * (1.4 + sqrt(flow_raw_sq)))
            * (0.12 * shore_prox * sqrt(flow_raw_sq));
        flow_eff = n_in * deflected_n + t_bank * (v_t * (1.0 - 0.36 * shore_prox) + eddy_wobble);
    }

    // 11x11 dispersive vertical derivative convolution V = G * h.
    var v_conv = 0.0;
    let tx = i32(lid.x) + KERNEL_R;
    let tz = i32(lid.y) + KERNEL_R;
    for (var dz: i32 = -KERNEL_R; dz <= KERNEL_R; dz = dz + 1) {
        let az = u32(abs(dz));
        let row_off = (tz + dz) * i32(TILE_DIM);
        for (var dx: i32 = -KERNEL_R; dx <= KERNEL_R; dx = dx + 1) {
            let ax = u32(abs(dx));
            let w_idx = az * 6u + ax;
            let weight = sim.kernel[w_idx >> 2u][w_idx & 3u];
            v_conv += weight * tile_h[u32(row_off + tx + dx)];
        }
    }

    // Semi-Lagrangian river advection only when local flow speed is nonzero so
    // calm lakes retain zero numerical diffusion.
    let flow_sq = dot(flow_eff, flow_eff);
    var prev_state: vec4<f32>;
    if flow_sq > 1e-4 {
        let adv_cells = clamp((flow_eff * dt) / texel_m, vec2<f32>(-1.5), vec2<f32>(1.5));
        let src_pos = vec2<f32>(f32(gx + shift.x), f32(gz + shift.y)) - adv_cells;
        prev_state = sample_state_bilinear(src_pos);
    } else {
        prev_state = load_state(gx + shift.x, gz + shift.y);
    }

    let h_curr = prev_state.x;
    let h_prev = prev_state.y;
    let depth_regime_deep = smoothstep(0.45, 1.25, depth_m);
    let foam_decay_rate = mix(1.85, 0.88, depth_regime_deep) + shore_prox * 0.42;
    var foam = prev_state.z * exp(-dt * foam_decay_rate);

    // Outer rolling boundary sponge + depth-stratified bottom & shoreline friction.
    let edge_margin = 24.0;
    let d_edge = min(
        min(f32(gx), f32(GRID_I - 1 - gx)),
        min(f32(gz), f32(GRID_I - 1 - gz)),
    );
    let edge_sponge = 1.0 - smoothstep(0.0, edge_margin, d_edge);
    let wet = smoothstep(0.008, 0.14, depth_m);
    let depth_speed = clamp(sqrt(depth_m / 0.85), 0.22, 1.0);
    let shallow_bed_drag = (1.0 - smoothstep(0.05, 0.62, depth_m)) * 1.95;
    let bank_friction = shore_prox * (1.25 + min(shore_slope, 1.5) * 0.95);
    let alpha = 0.34 + shallow_bed_drag + bank_friction + (1.0 - wet) * 5.2 + edge_sponge * 10.0;

    let a_dt = 0.5 * alpha * dt;
    let shoal_gain = 1.0 + 0.18 * shore_prox * smoothstep(0.05, 0.35, depth_m);
    let g_eff = sim.physics.x * depth_speed * shoal_gain;
    var h_next = (2.0 * h_curr - (1.0 - a_dt) * h_prev - g_eff * dt * dt * v_conv) / (1.0 + a_dt);

    // Bank shear excites delicate lapping capillary ripples where current brushes dry edges.
    if shore_prox > 0.12 && wet > 0.08 && flow_sq > 0.04 {
        let bank_ripple = sin(world_xz.x * 5.2 + world_xz.y * 4.4 - time * 4.8) * 0.5
            + cos(world_xz.x * 8.1 - world_xz.y * 7.3 + time * 5.6) * 0.5;
        h_next += bank_ripple * shore_prox * min(sqrt(flow_sq), 2.5) * 0.0018 * wet;
    }

    // Apply player / creature / waterfall / object water impulses scaled by local depth.
    let count = min(u32(sim.env_count.w + 0.5), 64u);
    let imp_depth_cap = clamp(depth_m / 0.52, 0.30, 1.0);
    let shallow_ring_boost = 1.0 + 0.35 * (1.0 - depth_regime_deep);
    var impulse_foam = 0.0;
    for (var i: u32 = 0u; i < count; i = i + 1u) {
        let imp = sim.impulses[i];
        let radius = max(imp.params.x, 0.18);
        let strength = imp.params.y * imp_depth_cap;
        let kind = imp.params.w;

        if kind < 0.5 {
            // Symmetric splash/fall cavity: crisp concentric crater + hollow annular crown foam.
            let delta = world_xz - imp.pos_vel.xy;
            let r2 = dot(delta, delta);
            let cut = radius * 2.35;
            if r2 < cut * cut {
                let u2 = r2 / (radius * radius);
                let cavity = (1.0 - 0.82 * u2) * exp(-u2 * 1.32);
                h_next -= strength * 1.15 * cavity;
                let r_norm = sqrt(u2);
                let crown_ring = exp(-pow((r_norm - 0.62) / 0.20, 2.0));
                let inner_faint = exp(-u2 * 2.2) * 0.16;
                impulse_foam += imp.params.z * (crown_ring * 0.92 * shallow_ring_boost + inner_faint * depth_regime_deep);
            }
        } else {
            // Directional moving wader / swimmer Kelvin dipole: bow rise + stern trough + crisp wake trail.
            let spd = length(imp.pos_vel.zw);
            let dir = select(vec2<f32>(1.0, 0.0), imp.pos_vel.zw / spd, spd > 1e-3);
            let perp = vec2<f32>(-dir.y, dir.x);
            let rel = world_xz - imp.pos_vel.xy;
            let off = dir * (radius * 0.52);
            let r2_bow = dot(rel - off, rel - off);
            let r2_stern = dot(rel + off, rel + off);
            let inv_r2 = 1.0 / (radius * radius * 0.68);
            let dipole = exp(-r2_bow * inv_r2) - 0.88 * exp(-r2_stern * inv_r2);
            h_next += dipole * strength * clamp(dt * 34.0, 0.22, 0.82);
            // Trail foam cleanly behind the stern + slight bow crest instead of enveloping the swimmer.
            let along = dot(rel, dir);
            let across = abs(dot(rel, perp));
            let wake_band = exp(-pow(across / max(radius * 0.42, 0.18), 2.0))
                * smoothstep(radius * 0.55, -radius * 0.15, along)
                * exp(-max(-along, 0.0) / max(radius * 1.25, 0.4));
            let bow_lip = exp(-r2_bow / max(radius * radius * 0.18, 0.04)) * 0.35;
            impulse_foam += imp.params.z * (wake_band * 0.72 + bow_lip) * dt * 5.2;
        }
    }
    h_next = clamp(h_next, -0.35, 0.35);

    // Continuous waterfall plunge pool boiling + frothy aeration.
    if plunge > 0.01 {
        let tick = floor(time * 18.0);
        let n1 = sin(world_xz.x * 2.3 + time * 11.0) * cos(world_xz.y * 2.1 - time * 9.5);
        let n2 = hash12(floor(world_xz * 2.0) + vec2<f32>(tick * 0.37, tick * 0.71)) - 0.5;
        h_next += plunge * (0.045 * n1 + 0.065 * n2) * dt * 60.0;
        impulse_foam = max(impulse_foam, plunge * (0.58 + 0.35 * (n1 * 0.5 + 0.5)));
    }

    // Live raindrop micro-crater impulses over open wet surfaces (subtler in very shallow puddles).
    let rain = clamp(sim.env_count.x, 0.0, 1.0);
    if rain > 0.02 && wet > 0.25 {
        let rain_step = floor(time * 20.0);
        let cell = floor(world_xz / 1.4);
        let rng_p = hash12(cell + vec2<f32>(rain_step * 1.13, rain_step * 2.41));
        if rng_p < rain * 0.32 {
            let jitter_x = hash12(cell * 3.1 + vec2<f32>(rain_step, 1.7));
            let jitter_z = hash12(cell * 5.7 + vec2<f32>(2.3, rain_step));
            let drop_center = (cell + vec2<f32>(0.15 + 0.70 * jitter_x, 0.15 + 0.70 * jitter_z)) * 1.4;
            let d2 = dot(world_xz - drop_center, world_xz - drop_center);
            if d2 < 0.14 {
                let rain_depth_gain = mix(0.65, 1.0, depth_regime_deep);
                h_next -= (0.014 + 0.018 * rain) * rain_depth_gain * exp(-d2 * 34.0);
            }
        }
    }

    // Steep wave crests and shoreline swash/breaking generate natural whitecap/splash foam.
    let v_vert = abs(h_next - h_curr) / max(dt, 0.004);
    let steep_foam = clamp((v_vert - 0.65) * 0.38, 0.0, 0.35) * dt * 6.0;
    let shore_break_foam = shore_prox
        * smoothstep(0.015, 0.14, depth_m)
        * clamp((v_vert * 0.85 + sqrt(flow_sq) * 0.32 - 0.28) * 0.55, 0.0, 0.42)
        * dt
        * 5.5;
    foam = clamp(foam + impulse_foam + steep_foam + shore_break_foam, 0.0, 1.0);

    let shore_damp = smoothstep(0.006, 0.11, depth_m);
    h_next *= shore_damp * (1.0 - edge_sponge * 0.35);
    h_next = sanitize(h_next);
    state_out[idx] = vec4<f32>(h_next, sanitize(h_curr), foam * wet, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn gradient_export(@builtin(global_invocation_id) gid: vec3<u32>) {
    let gx = i32(gid.x);
    let gz = i32(gid.y);
    let texel_m = max(sim.origin_size.w, 0.05);

    let xm = max(gx - 1, 0);
    let xp = min(gx + 1, GRID_I - 1);
    let zm = max(gz - 1, 0);
    let zp = min(gz + 1, GRID_I - 1);

    let c = state_out[u32(gz) * GRID_U + u32(gx)];
    let hl = state_out[u32(gz) * GRID_U + u32(xm)].x;
    let hr = state_out[u32(gz) * GRID_U + u32(xp)].x;
    let hd = state_out[u32(zm) * GRID_U + u32(gx)].x;
    let hu = state_out[u32(zp) * GRID_U + u32(gx)].x;

    let dh_dx = clamp((hr - hl) / (2.0 * texel_m), -1.8, 1.8);
    let dh_dz = clamp((hu - hd) / (2.0 * texel_m), -1.8, 1.8);

    textureStore(
        wave_out_tex,
        vec2<i32>(gx, gz),
        vec4<f32>(c.x, dh_dx, dh_dz, c.z),
    );
}

