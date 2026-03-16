from pathlib import Path


def read_mdoc_head(file_path: Path) -> tuple[str, str] | None:
    fnode = ""
    title = ""
    try:
        with file_path.open("r", encoding="utf-8") as handle:
            for raw_line in handle:
                line = raw_line.strip()
                lower = line.lower()
                if lower.startswith("@fnode:"):
                    fnode = line.split(":", 1)[1].strip()
                elif lower.startswith("@title:"):
                    title = line.split(":", 1)[1].strip()
                if fnode and title:
                    break
    except OSError:
        return None

    if not fnode or not title:
        return None
    return fnode, title
