mod algorithms;

pub use algorithms::{
    all_topo_depths, representative_cycles, strongly_connected_components, weak_component_sizes,
};

/// Make untrusted text inert before writing it to a terminal.
pub fn escape_terminal(value: &str) -> String {
    use std::fmt::Write;

    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            write!(escaped, "\\u{{{:x}}}", character as u32).expect("writing to a string");
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::escape_terminal;

    #[test]
    fn terminal_controls_are_escaped() {
        assert_eq!(
            escape_terminal("bad\u{1b}]0;title\u{7}"),
            "bad\\u{1b}]0;title\\u{7}"
        );
    }
}
