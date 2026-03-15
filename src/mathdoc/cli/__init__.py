from .parser import build_parser

__all__ = ["main"]


def main() -> int:
    args = build_parser().parse_args()
    return args.func(args)
