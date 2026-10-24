pub fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| {
        let d = x - y;
        d * d
    }).sum()
}