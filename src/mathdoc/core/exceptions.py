class DependencyCycleError(ValueError):
    def __init__(self, cycle: list[str]) -> None:
        super().__init__("dependency cycle detected")
        self.cycle = cycle
