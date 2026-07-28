use crate::core::NodeDegrees;

pub(super) fn ior(degrees: NodeDegrees) -> f64 {
    f64::from(degrees.in_degree).ln_1p() - f64::from(degrees.out_degree).ln_1p()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn degrees(in_degree: u32, out_degree: u32) -> NodeDegrees {
        NodeDegrees {
            in_degree,
            out_degree,
        }
    }

    #[test]
    fn ior_is_zero_for_balanced_degrees() {
        assert_eq!(ior(degrees(0, 0)), 0.0);
        assert_eq!(ior(degrees(7, 7)), 0.0);
    }

    #[test]
    fn ior_changes_sign_when_degrees_are_swapped() {
        let value = ior(degrees(3, 1));
        assert!(value > 0.0);
        assert!((value + ior(degrees(1, 3))).abs() < f64::EPSILON);
    }
}
