use nextdude_common::proto::v1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2d {
    pub x: f64,
    pub y: f64,
}

impl Vec2d {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn dist(self, other: Self) -> f64 {
        Self::new(self.x - other.x, self.y - other.y).len()
    }
}

#[derive(Debug, Clone)]
pub struct ComputedLinkPath {
    pub full_points: Vec<Vec2d>,
    pub src_half_points: Vec<Vec2d>,
    pub dst_half_points: Vec<Vec2d>,
    pub midpoint: Vec2d,
    pub default_handle: Vec2d,
    pub active_handle: Vec2d,
}

/// Computes the intersection point on a node's outer boundary along the ray toward `target`.
pub fn node_boundary_point(
    center: Vec2d,
    half_w: f64,
    half_h: f64,
    shape: v1::NodeShape,
    target: Vec2d,
) -> Vec2d {
    let dx = target.x - center.x;
    let dy = target.y - center.y;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1e-4 {
        return center;
    }
    let hw = half_w.max(8.0);
    let hh = half_h.max(8.0);

    match shape {
        v1::NodeShape::Circle => {
            let r = hw.min(hh);
            Vec2d::new(center.x + (dx / dist) * r, center.y + (dy / dist) * r)
        }
        _ => {
            let scale_x = if dx.abs() > 1e-6 { hw / dx.abs() } else { f64::INFINITY };
            let scale_y = if dy.abs() > 1e-6 { hh / dy.abs() } else { f64::INFINITY };
            let t = scale_x.min(scale_y);
            Vec2d::new(center.x + dx * t, center.y + dy * t)
        }
    }
}

/// Computes symmetric perpendicular offset for parallel links sharing the same endpoints.
pub fn parallel_link_offset(index: usize, total: usize, spacing: f64) -> f64 {
    if total <= 1 {
        return 0.0;
    }
    let center = (total as f64 - 1.0) / 2.0;
    (index as f64 - center) * spacing
}

#[allow(clippy::too_many_arguments)]
pub fn compute_elastic_link_path(
    src_center: Vec2d,
    src_half_w: f64,
    src_half_h: f64,
    src_shape: v1::NodeShape,
    dst_center: Vec2d,
    dst_half_w: f64,
    dst_half_h: f64,
    dst_shape: v1::NodeShape,
    style: v1::LinkStyle,
    bend_points: &[Vec2d],
    parallel_idx: usize,
    parallel_total: usize,
) -> ComputedLinkPath {
    let dx = dst_center.x - src_center.x;
    let dy = dst_center.y - src_center.y;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let nx = -dy / len;
    let ny = dx / len;
    let fan_offset = parallel_link_offset(parallel_idx, parallel_total, 26.0);

    let mid_raw = Vec2d::new(
        (src_center.x + dst_center.x) * 0.5,
        (src_center.y + dst_center.y) * 0.5,
    );
    let auto_ctrl = Vec2d::new(mid_raw.x + nx * fan_offset, mid_raw.y + ny * fan_offset);

    let active_handle = bend_points.first().copied().unwrap_or(auto_ctrl);

    let start_aim = if bend_points.is_empty() && fan_offset.abs() < 0.1 {
        dst_center
    } else {
        active_handle
    };
    let end_aim = if bend_points.is_empty() && fan_offset.abs() < 0.1 {
        src_center
    } else {
        *bend_points.last().unwrap_or(&active_handle)
    };

    let p0 = node_boundary_point(src_center, src_half_w, src_half_h, src_shape, start_aim);
    let p_end = node_boundary_point(dst_center, dst_half_w, dst_half_h, dst_shape, end_aim);

    let full_points = match style {
        v1::LinkStyle::Straight if bend_points.is_empty() && fan_offset.abs() < 0.1 => {
            vec![p0, p_end]
        }
        v1::LinkStyle::Straight => {
            let mut pts = Vec::with_capacity(2 + bend_points.len());
            pts.push(p0);
            if bend_points.is_empty() {
                pts.push(auto_ctrl);
            } else {
                pts.extend_from_slice(bend_points);
            }
            pts.push(p_end);
            pts
        }
        v1::LinkStyle::OrthogonalRounded => {
            build_rounded_orthogonal(p0, p_end, active_handle, 12.0)
        }
        _ => {
            // Smooth spline / quadratic Bezier (or multi-bend Catmull-Rom-like quadratic segments)
            if bend_points.len() <= 1 {
                tessellate_quadratic_bezier(p0, active_handle, p_end, 24)
            } else {
                tessellate_multi_bend_spline(p0, bend_points, p_end, 16)
            }
        }
    };

    let (src_half_points, dst_half_points, midpoint) = split_polyline_at_midpoint(&full_points);

    ComputedLinkPath {
        full_points,
        src_half_points,
        dst_half_points,
        midpoint,
        default_handle: auto_ctrl,
        active_handle,
    }
}

fn tessellate_quadratic_bezier(p0: Vec2d, p1: Vec2d, p2: Vec2d, segments: usize) -> Vec<Vec2d> {
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = (i as f64) / (segments as f64);
        let omt = 1.0 - t;
        let x = omt * omt * p0.x + 2.0 * omt * t * p1.x + t * t * p2.x;
        let y = omt * omt * p0.y + 2.0 * omt * t * p1.y + t * t * p2.y;
        pts.push(Vec2d::new(x, y));
    }
    pts
}

fn tessellate_multi_bend_spline(
    start: Vec2d,
    bends: &[Vec2d],
    end: Vec2d,
    segs_per_span: usize,
) -> Vec<Vec2d> {
    let mut ctrl = Vec::with_capacity(bends.len() + 2);
    ctrl.push(start);
    ctrl.extend_from_slice(bends);
    ctrl.push(end);

    let mut out = Vec::new();
    for i in 0..ctrl.len() - 1 {
        let p0 = if i == 0 { ctrl[0] } else { ctrl[i - 1] };
        let p1 = ctrl[i];
        let p2 = ctrl[i + 1];
        let p3 = if i + 2 < ctrl.len() {
            ctrl[i + 2]
        } else {
            ctrl[ctrl.len() - 1]
        };
        for s in 0..segs_per_span {
            let t = (s as f64) / (segs_per_span as f64);
            out.push(catmull_rom(p0, p1, p2, p3, t));
        }
    }
    out.push(end);
    out
}

fn catmull_rom(p0: Vec2d, p1: Vec2d, p2: Vec2d, p3: Vec2d, t: f64) -> Vec2d {
    let t2 = t * t;
    let t3 = t2 * t;
    let x = 0.5
        * ((2.0 * p1.x)
            + (-p0.x + p2.x) * t
            + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
            + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3);
    let y = 0.5
        * ((2.0 * p1.y)
            + (-p0.y + p2.y) * t
            + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
            + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
    Vec2d::new(x, y)
}

fn build_rounded_orthogonal(p0: Vec2d, p2: Vec2d, handle: Vec2d, radius: f64) -> Vec<Vec2d> {
    let c1 = Vec2d::new(handle.x, p0.y);
    let c2 = Vec2d::new(handle.x, p2.y);
    let d1 = p0.dist(c1);
    let d2 = c1.dist(c2);
    let d3 = c2.dist(p2);
    let r = radius.min(d1 * 0.4).min(d2 * 0.4).min(d3 * 0.4);
    if r < 2.0 {
        return vec![p0, c1, c2, p2];
    }

    let dir_x = (c1.x - p0.x).signum();
    let dir_y = (c2.y - c1.y).signum();
    let dir_x2 = (p2.x - c2.x).signum();

    let a1 = Vec2d::new(c1.x - dir_x * r, c1.y);
    let b1 = Vec2d::new(c1.x, c1.y + dir_y * r);
    let a2 = Vec2d::new(c2.x, c2.y - dir_y * r);
    let b2 = Vec2d::new(c2.x + dir_x2 * r, c2.y);

    let mut out = Vec::new();
    out.push(p0);
    out.extend(tessellate_quadratic_bezier(a1, c1, b1, 6));
    out.extend(tessellate_quadratic_bezier(a2, c2, b2, 6));
    out.push(p2);
    out
}

pub fn split_polyline_at_midpoint(pts: &[Vec2d]) -> (Vec<Vec2d>, Vec<Vec2d>, Vec2d) {
    if pts.len() < 2 {
        let p = pts.first().copied().unwrap_or(Vec2d::new(0.0, 0.0));
        return (vec![p], vec![p], p);
    }
    let mut seg_lens = Vec::with_capacity(pts.len() - 1);
    let mut total = 0.0;
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        seg_lens.push(l);
        total += l;
    }
    let half = total * 0.5;
    let mut acc = 0.0;
    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut mid = pts[pts.len() / 2];

    for (idx, w) in pts.windows(2).enumerate() {
        let l = seg_lens[idx];
        if acc + l >= half && second.is_empty() {
            let t = if l > 1e-6 { (half - acc) / l } else { 0.5 };
            mid = Vec2d::new(
                w[0].x + (w[1].x - w[0].x) * t,
                w[0].y + (w[1].y - w[0].y) * t,
            );
            first.push(w[0]);
            first.push(mid);
            second.push(mid);
            second.push(w[1]);
        } else if second.is_empty() {
            first.push(w[0]);
        } else {
            second.push(w[1]);
        }
        acc += l;
    }
    if first.is_empty() {
        first.push(pts[0]);
        first.push(mid);
    }
    if second.is_empty() {
        second.push(mid);
        second.push(*pts.last().unwrap_or(&mid));
    }
    (first, second, mid)
}

/// Computes minimum Euclidean distance from point `p` to a tessellated link polyline.
pub fn distance_to_polyline(p: Vec2d, pts: &[Vec2d]) -> f64 {
    if pts.is_empty() {
        return f64::INFINITY;
    }
    if pts.len() == 1 {
        return p.dist(pts[0]);
    }
    let mut best = f64::INFINITY;
    for w in pts.windows(2) {
        best = best.min(distance_to_segment(p, w[0], w[1]));
    }
    best
}

fn distance_to_segment(p: Vec2d, a: Vec2d, b: Vec2d) -> f64 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq < 1e-9 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq).clamp(0.0, 1.0);
    let proj = Vec2d::new(a.x + t * abx, a.y + t * aby);
    p.dist(proj)
}

/// Simple vertical stagger overlap avoidance for mid-link labels.
pub fn avoid_label_overlaps(centers: &mut [Vec2d], label_w: f64, label_h: f64) {
    for i in 0..centers.len() {
        for j in 0..i {
            if (centers[i].x - centers[j].x).abs() < label_w
                && (centers[i].y - centers[j].y).abs() < label_h
            {
                centers[i].y = centers[j].y + label_h + 4.0;
            }
        }
    }
}
