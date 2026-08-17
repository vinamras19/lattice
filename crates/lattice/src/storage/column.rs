use super::gorilla;

pub fn encode(points: &[(i64, f64)]) -> Vec<u8> {
    gorilla::encode(points)
}

pub fn decode(data: &[u8], count: usize) -> Vec<(i64, f64)> {
    gorilla::decode(data, count)
}
