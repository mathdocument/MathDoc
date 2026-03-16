"""Dependency graph package."""

__all__ = ["DepGraph"]


def __getattr__(name: str) -> object:
    if name == "DepGraph":
        from .graph import DepGraph

        return DepGraph
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
