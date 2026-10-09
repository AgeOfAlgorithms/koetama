//! How long a room_seed takes to become a room (scrypt, once per seed).
fn main() {
    let t0 = std::time::Instant::now();
    let (room, _) = kd_common::feed::room_from_seed("lobby 42 + its password");
    let first = t0.elapsed();
    let t1 = std::time::Instant::now();
    kd_common::feed::room_from_seed("lobby 42 + its password");
    println!("room {room}: {:.0} ms the first time, {:.3} ms again", first.as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3);
}
