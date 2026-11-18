//! Rectangle geometry in the grid and in screen pixels: intersection, point test, offset and client area
//! (re/menu-system 3.2, 8.4.1). The only place for `IntersectRect` in the crate.
//!
//! Rectangles are `[left, top, right, bottom]`; right and bottom are not part of the rectangle (Win32 `RECT`).

/// `IntersectRect`: intersection of two rectangles, `None` if it is empty (`IsRectEmpty`: width or height <= 0).
pub fn intersect(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// `PtInRect`: left and top belong to the rectangle, right and bottom do not.
pub fn contains(r: [i32; 4], x: i32, y: i32) -> bool {
    r[0] <= x && x < r[2] && r[1] <= y && y < r[3]
}

/// Rectangle shifted by `[dx, dy]` (32-bit overflow as in the original).
pub fn offset(r: [i32; 4], [dx, dy]: [i32; 2]) -> [i32; 4] {
    [
        r[0].wrapping_add(dx),
        r[1].wrapping_add(dy),
        r[2].wrapping_add(dx),
        r[3].wrapping_add(dy),
    ]
}

/// Client area relative to the top-left corner (`FUN_004598D0`): local rectangle `(0, 0, w, h)` of `rect`, inset by
/// the `CLIENTAREA` of the loaded `.bkg`, intersected with the local rectangle; an empty intersection yields
/// `(0, 0, 0, 0)` (`IntersectRect`). Without a `.bkg` (`client_area = None`) there is no inset.
pub fn client_rel(rect: [i32; 4], client_area: Option<[i32; 4]>) -> [i32; 4] {
    let w = rect[2].wrapping_sub(rect[0]);
    let h = rect[3].wrapping_sub(rect[1]);
    let local = [0, 0, w, h];
    let inset = match client_area {
        Some([l, t, r, b]) => [l, t, w.wrapping_sub(r), h.wrapping_sub(b)],
        None => local,
    };
    intersect(inset, local).unwrap_or([0; 4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geom_matches_ptinrect_and_intersectrect() {
        let r = [10, 20, 30, 40];
        // PtInRect: left/top inside, right/bottom outside.
        assert!(contains(r, 10, 20));
        assert!(contains(r, 29, 39));
        assert!(!contains(r, 30, 39));
        assert!(!contains(r, 29, 40));
        assert!(!contains(r, 9, 25));
        assert!(!contains(r, 15, 19));
        // An empty rectangle contains nothing.
        assert!(!contains([5, 5, 5, 9], 5, 6));
        // IntersectRect: intersection; adjacent rectangles do not intersect.
        assert_eq!(intersect(r, [0, 0, 15, 25]), Some([10, 20, 15, 25]));
        assert_eq!(intersect(r, r), Some(r));
        assert_eq!(intersect(r, [30, 20, 40, 40]), None);
        assert_eq!(intersect(r, [10, 40, 30, 50]), None);
        assert_eq!(intersect([0, 0, 0, 10], [0, 0, 10, 10]), None);
        // Swapped corners count as empty.
        assert_eq!(intersect([30, 40, 10, 20], [0, 0, 100, 100]), None);
    }
}
