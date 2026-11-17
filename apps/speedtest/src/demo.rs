//! A made-up test's results (`OLO_SPEEDTEST_DEMO=1`: the window shows
//! them and connects to nothing), for the layout lint and for pictures:
//! a fibre line over Wi-Fi with BBRv3 sending, every figure there and
//! each as long as it gets.

use crate::{
    engine::{Bbr, Latency, Ours, Place, Results, Theirs, Transfer},
    view::Link,
};

/// The rate each tenth of a second for `secs`: up to `top` in the first
/// second, then about it.
fn series(top: f64, secs: f64, wobble: f64) -> Vec<(f32, f32)> {
    (1..=(secs * 10.0) as usize)
        .map(|i| {
            let t = i as f64 / 10.0;
            let ramp = (t / 1.2).min(1.0).powf(0.6);
            let v = top * ramp * (1.0 + wobble * ((i as f64 * 0.9).sin() * 0.6 + (i as f64 * 2.3).cos() * 0.4));
            (t as f32, v as f32)
        })
        .collect()
}

pub fn results() -> Results {
    let latency = |min: f64, median: f64, p90: f64, jitter: f64, samples: usize| Latency { samples, min, median, mean: median * 1.05, p90, max: p90 * 1.3, jitter };
    let down = Transfer {
        secs: 8.0,
        bytes: 938_525_947,
        mbps: 941.3,
        peak_mbps: 1012.0,
        series: series(960.0, 8.0, 0.06),
        streams: 4,
        latency: latency(9.8, 23.4, 41.0, 6.1, 38),
        ours: Ours {
            congestion_control: "bbr3".into(),
            mss: 1388,
            pmtu: 1500,
            min_rtt_ms: 4.6,
            rtt_ms: 22.4,
            bytes_received: 939_483_162,
            out_of_order: 11_547,
            rcv_rtt_ms: 71.2,
            rcv_window: 44_221_440,
            ..Ours::default()
        },
        theirs: Some(Theirs { rtt_ms: 55.3, min_rtt_ms: 4.1, rttvar_ms: 3.1, sent: 671_293, received: 27_992, lost: 12, retransmissions: 117, delivery_mbps: 991.3, cwnd: 8644 }),
    };
    let up = Transfer {
        secs: 8.0,
        bytes: 600_051_192,
        mbps: 622.9,
        peak_mbps: 716.5,
        series: series(640.0, 8.0, 0.09),
        streams: 4,
        latency: latency(10.2, 14.8, 22.9, 3.0, 39),
        ours: Ours {
            congestion_control: "bbr3".into(),
            mss: 1388,
            pmtu: 1500,
            min_rtt_ms: 4.5,
            rtt_ms: 9.7,
            rttvar_ms: 3.1,
            bytes_sent: 608_891_112,
            bytes_retransmitted: 141_164,
            retransmissions: 103,
            timeouts: 0,
            spurious: 3,
            reordering: 23,
            cwnd: 1026,
            pacing_mbps: 672.2,
            delivery_mbps: 631.1,
            rwnd_limited_pct: 1.9,
            sndbuf_limited_pct: 0.4,
            bbr: Some(Bbr { bandwidth_mbps: 648.0, min_rtt_ms: 4.5, phases: vec!["cruise".into(), "probe up".into(), "cruise".into(), "cruise".into()] }),
            ..Ours::default()
        },
        theirs: Some(Theirs { rtt_ms: 16.9, min_rtt_ms: 4.0, rttvar_ms: 11.9, sent: 34_619, received: 399_065, lost: 0, retransmissions: 0, delivery_mbps: 11.1, cwnd: 226 }),
    };
    Results {
        when: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(190),
        server: "speed.cloudflare.com".into(),
        address: "2606:4700:4400::ac40:9b55".into(),
        ipv6: true,
        place: Place { colo: "SEA".into(), city: "Seattle".into(), country: "US".into(), asn: "64496".into() },
        dns_ms: 10.5,
        connect_ms: 7.1,
        tls_ms: 36.9,
        idle: latency(9.6, 11.2, 13.5, 1.8, 20),
        idle_tcp_rtt_ms: 4.8,
        download: Some(down),
        upload: Some(up),
        stopped: false,
    }
}

/// The round trips of [`results`]' test, by seconds into it: idle, then
/// while each way ran.
pub fn latency() -> Vec<(f32, f32)> {
    (0..90)
        .map(|i| {
            let t = 0.4 + i as f32 * 0.2;
            let base = if t < 1.6 {
                11.0
            } else if t < 9.8 {
                23.0
            } else {
                15.0
            };
            (t, base + ((i as f32 * 1.7).sin() * 0.35 + (i as f32 * 0.6).cos() * 0.2) * base * 0.5)
        })
        .collect()
}

pub fn link() -> Link {
    Link { kind: "Wi-Fi".into(), interface: "wlan0".into(), name: Some("The House on the Hill 5G".into()), rate_mbps: Some(1921), band: Some("6 GHz".into()), signal: Some(78), metered: false }
}
