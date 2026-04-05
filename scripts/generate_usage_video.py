#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import threading
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterable

from PIL import Image, ImageDraw, ImageFont


FPS = 30
WIDTH = 1280
HEIGHT = 720
PADDING = 36
WINDOW_RADIUS = 22
CONTENT_PADDING_X = 34
CONTENT_PADDING_Y = 34
TITLE_BAR_HEIGHT = 40
FONT_SIZE = 24
LINE_SPACING = 10
MAX_COLUMNS = 82
MAX_VISIBLE_LINES = 17
PROMPT = "$ "
BACKGROUND = "#0b1020"
WINDOW = "#111827"
TITLE_BAR = "#1f2937"
TEXT = "#e5e7eb"
MUTED = "#94a3b8"
ACCENT = "#22c55e"
DISPLAY_BASE_URL = "http://127.0.0.1:8080"
INTRO_LINES = [
    "transfer-rs usage demo",
    "upload, download, and delete against a local transfer-compatible server",
    "",
    "generated from real CLI output",
]
TITLE = "transfer-rs usage demo"
STATE_PREFIX = "transfer-rs-state:"


def log_status(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


@dataclass
class DemoStep:
    display_command: str
    output_lines: list[str]


class DemoServer(ThreadingHTTPServer):
    def __init__(self, server_address: tuple[str, int]):
        super().__init__(server_address, DemoHandler)
        self.files: dict[str, bytes] = {}

    @property
    def base_url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}"


class DemoHandler(BaseHTTPRequestHandler):
    server: DemoServer

    def do_PUT(self) -> None:
        path = self.path.lstrip("/")
        length = int(self.headers.get("Content-Length", "0"))
        self.server.files[path] = self.rfile.read(length)
        download_url = f"{self.server.base_url}/{path}"
        delete_url = f"{self.server.base_url}/delete/{path}"
        body = f"{download_url}\n".encode("utf-8")

        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Url-Delete", delete_url)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        path = self.path.lstrip("/")
        if path not in self.server.files:
            self.send_error(404)
            return

        payload = self.server.files[path]
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_DELETE(self) -> None:
        prefix = "/delete/"
        if not self.path.startswith(prefix):
            self.send_error(404)
            return

        path = self.path[len(prefix) :]
        if path in self.server.files:
            del self.server.files[path]

        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, format: str, *args: object) -> None:
        return


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate terminal-style usage demo media for transfer-rs.")
    parser.add_argument(
        "--output",
        default="demo/usage-demo.mp4",
        help="Output path for the generated MP4, relative to the repository root.",
    )
    parser.add_argument(
        "--gif-output",
        help="Optional output path for the generated GIF, relative to the repository root. Defaults to the MP4 path with a .gif suffix.",
    )
    return parser.parse_args()


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def load_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    candidates = [
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Supplemental/Menlo.ttc",
        "/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
    ]
    for candidate in candidates:
        if Path(candidate).exists():
            return ImageFont.truetype(candidate, size=size)
    return ImageFont.load_default()


def ensure_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise SystemExit(f"required tool not found: {name}")


def run_command(command: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        check=True,
    )


def embedded_state_text(payload: dict[str, str]) -> str:
    return STATE_PREFIX + json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def parse_embedded_state(raw_value: str | None) -> dict[str, str] | None:
    if raw_value is None or not raw_value.startswith(STATE_PREFIX):
        return None

    try:
        payload = json.loads(raw_value[len(STATE_PREFIX) :])
    except json.JSONDecodeError:
        return None

    if not isinstance(payload, dict):
        return None

    return {str(key): str(value) for key, value in payload.items()}


def expected_embedded_state(
    repository_root: Path,
    gif_output_path: Path,
    *,
    preflight_signature: str | None = None,
    render_signature: str | None = None,
) -> dict[str, str]:
    payload = {
        "version": "1",
        "gif_output": str(gif_output_path.relative_to(repository_root)),
    }
    if preflight_signature is not None:
        payload["preflight_signature"] = preflight_signature
    if render_signature is not None:
        payload["render_signature"] = render_signature
    return payload


def state_matches(actual: dict[str, str] | None, expected: dict[str, str]) -> bool:
    if actual is None:
        return False
    return all(actual.get(key) == value for key, value in expected.items())


def read_video_embedded_state(media_path: Path) -> dict[str, str] | None:
    try:
        result = subprocess.run(
            [
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "format_tags=comment",
                "-of",
                "json",
                str(media_path),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None

    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None

    tags = payload.get("format", {}).get("tags", {})
    comment = tags.get("comment") if isinstance(tags, dict) else None
    return parse_embedded_state(comment if isinstance(comment, str) else None)


def gif_global_color_table_size(data: bytes) -> int:
    if len(data) < 13 or data[:6] not in {b"GIF87a", b"GIF89a"}:
        raise ValueError("invalid gif header")

    packed = data[10]
    if not packed & 0x80:
        return 0

    return 3 * (2 ** ((packed & 0x07) + 1))


def iter_gif_extension_blocks(data: bytes) -> Iterable[tuple[int, bytes]]:
    index = 13 + gif_global_color_table_size(data)
    while index < len(data):
        block_type = data[index]
        if block_type == 0x3B:
            return

        if block_type == 0x21:
            if index + 1 >= len(data):
                raise ValueError("truncated gif extension label")

            label = data[index + 1]
            index += 2
            chunks: list[bytes] = []
            while True:
                if index >= len(data):
                    raise ValueError("truncated gif extension block")

                block_size = data[index]
                index += 1
                if block_size == 0:
                    break

                if index + block_size > len(data):
                    raise ValueError("truncated gif extension payload")

                chunks.append(data[index : index + block_size])
                index += block_size

            yield label, b"".join(chunks)
            continue

        if block_type != 0x2C:
            raise ValueError("unsupported gif block")

        if index + 10 > len(data):
            raise ValueError("truncated gif image descriptor")

        packed = data[index + 9]
        index += 10
        if packed & 0x80:
            local_color_table_size = 3 * (2 ** ((packed & 0x07) + 1))
            if index + local_color_table_size > len(data):
                raise ValueError("truncated gif local color table")
            index += local_color_table_size

        if index >= len(data):
            raise ValueError("truncated gif image data")

        index += 1
        while True:
            if index >= len(data):
                raise ValueError("truncated gif image data block")

            block_size = data[index]
            index += 1
            if block_size == 0:
                break

            if index + block_size > len(data):
                raise ValueError("truncated gif image data payload")
            index += block_size


def read_gif_embedded_state(media_path: Path) -> dict[str, str] | None:
    try:
        data = media_path.read_bytes()
        for label, payload in iter_gif_extension_blocks(data):
            if label != 0xFE:
                continue

            try:
                raw_value = payload.decode("ascii")
            except UnicodeDecodeError:
                continue

            state = parse_embedded_state(raw_value)
            if state is not None:
                return state
    except (OSError, ValueError):
        return None

    return None


def gif_comment_extension(raw_value: str) -> bytes:
    payload = raw_value.encode("ascii")
    chunks = [b"\x21\xFE"]
    for start in range(0, len(payload), 255):
        chunk = payload[start : start + 255]
        chunks.append(bytes((len(chunk),)))
        chunks.append(chunk)
    chunks.append(b"\x00")
    return b"".join(chunks)


def embed_gif_state(media_path: Path, payload: dict[str, str]) -> None:
    data = media_path.read_bytes()
    insertion_offset = 13 + gif_global_color_table_size(data)
    state_block = gif_comment_extension(embedded_state_text(payload))
    media_path.write_bytes(data[:insertion_offset] + state_block + data[insertion_offset:])


def read_embedded_state(media_path: Path) -> dict[str, str] | None:
    if media_path.suffix.lower() == ".gif":
        return read_gif_embedded_state(media_path)
    return read_video_embedded_state(media_path)


def read_shared_embedded_state(output_path: Path, gif_output_path: Path) -> dict[str, str] | None:
    if not output_path.exists() or not gif_output_path.exists():
        return None

    output_state = read_embedded_state(output_path)
    gif_state = read_embedded_state(gif_output_path)
    if output_state is None or gif_state is None or output_state != gif_state:
        return None

    return output_state


def normalize_demo_text(value: str, actual_base_url: str) -> str:
    return value.replace(actual_base_url, DISPLAY_BASE_URL)


def normalize_demo_lines(lines: Iterable[str], actual_base_url: str) -> list[str]:
    return [normalize_demo_text(line, actual_base_url) for line in lines]


def tracked_input_paths(repository_root: Path) -> list[Path]:
    paths = [repository_root / "Cargo.toml", repository_root / "Cargo.lock", Path(__file__).resolve()]
    src_root = repository_root / "src"
    if src_root.exists():
        paths.extend(path for path in src_root.rglob("*.rs") if path.is_file())
    return sorted(paths)


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def preflight_signature(repository_root: Path) -> str:
    payload = [
        {
            "path": str(path.relative_to(repository_root)),
            "digest": file_digest(path),
        }
        for path in tracked_input_paths(repository_root)
    ]
    serialized = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


def should_regenerate_demo(
    repository_root: Path,
    output_path: Path,
    gif_output_path: Path,
    preflight_signature_value: str,
) -> bool:
    state = read_shared_embedded_state(output_path, gif_output_path)
    if state is None:
        return True

    expected = expected_embedded_state(
        repository_root,
        gif_output_path,
        preflight_signature=preflight_signature_value,
    )
    return not state_matches(state, expected)


def create_demo_steps(repository_root: Path) -> list[DemoStep]:
    ensure_tool("cargo")
    ensure_tool("ffmpeg")

    log_status("building transfer-rs demo binary")
    subprocess.run(["cargo", "build", "--quiet"], cwd=repository_root, check=True)
    binary = repository_root / "target" / "debug" / "transfer-rs"

    with tempfile.TemporaryDirectory(prefix="transfer-rs-demo-home-") as home_dir, tempfile.TemporaryDirectory(
        prefix="transfer-rs-demo-work-"
    ) as work_dir:
        home_path = Path(home_dir)
        work_path = Path(work_dir)
        source_file = work_path / "source.txt"
        source_file.write_text("plain payload\n", encoding="utf-8")

        env = os.environ.copy()
        env["HOME"] = str(home_path)

        log_status("starting local demo server")
        server = DemoServer(("127.0.0.1", 0))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()

        try:
            log_status("capturing --help output")
            help_result = run_command([str(binary), "--help"], cwd=work_path, env=env)
            help_lines = help_result.stdout.strip().splitlines()
            if len(help_lines) > 13:
                help_lines = help_lines[:12] + ["..."]

            log_status("capturing upload output")
            upload_result = run_command(
                [str(binary), "--server", server.base_url, "upload", "source.txt"],
                cwd=work_path,
                env=env,
            )

            source_file.unlink()

            download_url = f"{server.base_url}/source.txt"
            log_status("capturing download output")
            download_result = run_command([str(binary), "download", download_url], cwd=work_path, env=env)
            log_status("capturing file contents")
            cat_result = run_command(["cat", "source.txt"], cwd=work_path, env=env)
            log_status("capturing delete output")
            delete_result = run_command([str(binary), "delete", download_url], cwd=work_path, env=env)
        finally:
            log_status("stopping local demo server")
            server.shutdown()
            server.server_close()
            server_thread.join(timeout=2)

    return [
        DemoStep("transfer-rs --help", help_lines),
        DemoStep(
            f"transfer-rs --server {DISPLAY_BASE_URL} upload source.txt",
            normalize_demo_lines(upload_result.stdout.strip().splitlines(), server.base_url),
        ),
        DemoStep("rm source.txt", []),
        DemoStep(
            f"transfer-rs download {normalize_demo_text(download_url, server.base_url)}",
            normalize_demo_lines(download_result.stdout.strip().splitlines(), server.base_url),
        ),
        DemoStep("cat source.txt", cat_result.stdout.strip().splitlines()),
        DemoStep(
            f"transfer-rs delete {normalize_demo_text(download_url, server.base_url)}",
            normalize_demo_lines(delete_result.stdout.strip().splitlines(), server.base_url),
        ),
    ]


def wrap_command(command: str) -> list[str]:
    wrapped = textwrap.wrap(PROMPT + command, width=MAX_COLUMNS, subsequent_indent=" " * len(PROMPT))
    return wrapped or [PROMPT]


def wrap_output(lines: Iterable[str]) -> list[str]:
    wrapped_lines: list[str] = []
    for line in lines:
        chunks = textwrap.wrap(line, width=MAX_COLUMNS) or [""]
        wrapped_lines.extend(chunks)
    return wrapped_lines


def append_step_lines(transcript: list[str], step: DemoStep) -> None:
    transcript.extend(wrap_command(step.display_command))
    transcript.extend(wrap_output(step.output_lines))


def draw_frame(lines: list[str], title: str, frame_path: Path, font: ImageFont.ImageFont) -> None:
    image = Image.new("RGB", (WIDTH, HEIGHT), BACKGROUND)
    draw = ImageDraw.Draw(image)

    window_left = PADDING
    window_top = PADDING
    window_right = WIDTH - PADDING
    window_bottom = HEIGHT - PADDING

    draw.rounded_rectangle(
        (window_left, window_top, window_right, window_bottom),
        radius=WINDOW_RADIUS,
        fill=WINDOW,
    )
    draw.rounded_rectangle(
        (window_left, window_top, window_right, window_top + TITLE_BAR_HEIGHT),
        radius=WINDOW_RADIUS,
        fill=TITLE_BAR,
    )
    draw.rectangle(
        (window_left, window_top + TITLE_BAR_HEIGHT - WINDOW_RADIUS, window_right, window_top + TITLE_BAR_HEIGHT),
        fill=TITLE_BAR,
    )

    circle_y = window_top + TITLE_BAR_HEIGHT // 2
    for index, color in enumerate(("#fb7185", "#fbbf24", "#34d399")):
        circle_x = window_left + 20 + index * 18
        draw.ellipse((circle_x, circle_y - 6, circle_x + 12, circle_y + 6), fill=color)

    title_width = draw.textbbox((0, 0), title, font=font)[2]
    draw.text(((WIDTH - title_width) / 2, window_top + 8), title, font=font, fill=MUTED)

    line_height = FONT_SIZE + LINE_SPACING
    start_y = window_top + TITLE_BAR_HEIGHT + CONTENT_PADDING_Y
    visible_lines = lines[-MAX_VISIBLE_LINES:]
    for index, line in enumerate(visible_lines):
        y = start_y + index * line_height
        fill = ACCENT if line.startswith(PROMPT) else TEXT
        draw.text((window_left + CONTENT_PADDING_X, y), line, font=font, fill=fill)

    image.save(frame_path)


def run_ffmpeg(command: list[str]) -> None:
    subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
    )


def replace_if_changed(candidate_path: Path, destination_path: Path) -> None:
    if destination_path.exists() and destination_path.read_bytes() == candidate_path.read_bytes():
        candidate_path.unlink()
        return

    candidate_path.replace(destination_path)


def render_signature(steps: list[DemoStep]) -> str:
    payload = {
        "intro_lines": INTRO_LINES,
        "title": TITLE,
        "fps": FPS,
        "width": WIDTH,
        "height": HEIGHT,
        "padding": PADDING,
        "window_radius": WINDOW_RADIUS,
        "content_padding_x": CONTENT_PADDING_X,
        "content_padding_y": CONTENT_PADDING_Y,
        "title_bar_height": TITLE_BAR_HEIGHT,
        "font_size": FONT_SIZE,
        "line_spacing": LINE_SPACING,
        "max_columns": MAX_COLUMNS,
        "max_visible_lines": MAX_VISIBLE_LINES,
        "prompt": PROMPT,
        "background": BACKGROUND,
        "window": WINDOW,
        "title_bar": TITLE_BAR,
        "text": TEXT,
        "muted": MUTED,
        "accent": ACCENT,
        "steps": [
            {
                "display_command": step.display_command,
                "output_lines": step.output_lines,
            }
            for step in steps
        ],
    }
    serialized = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


def should_render(
    steps: list[DemoStep],
    repository_root: Path,
    output_path: Path,
    gif_output_path: Path,
    preflight_signature_value: str,
) -> tuple[bool, str]:
    signature = render_signature(steps)
    state = read_shared_embedded_state(output_path, gif_output_path)

    if state is None:
        return True, signature

    expected = expected_embedded_state(
        repository_root,
        gif_output_path,
        preflight_signature=preflight_signature_value,
        render_signature=signature,
    )
    if not state_matches(state, expected):
        return True, signature

    return False, signature


def total_render_frames(steps: list[DemoStep]) -> int:
    total_frames = FPS * 2
    for step in steps:
        total_frames += len(step.display_command)
        total_frames += max(6, FPS // 4)
        total_frames += len(wrap_output(step.output_lines)) * max(7, FPS // 5)
        total_frames += FPS // 2
    total_frames += FPS * 2
    return total_frames


class RenderProgress:
    def __init__(self, total_frames: int):
        self.total_frames = max(total_frames, 1)
        self.current_frame = 0
        self.last_reported_frame = -1
        self.is_tty = sys.stderr.isatty()

    def advance(self) -> None:
        self.current_frame = min(self.total_frames, self.current_frame + 1)
        self.report()

    def report(self, *, force: bool = False) -> None:
        if not force:
            if self.is_tty and self.current_frame == self.last_reported_frame:
                return
            if not self.is_tty and self.current_frame < self.total_frames:
                if self.current_frame % 10 != 0 and self.current_frame != 1:
                    return

        percent = (self.current_frame / self.total_frames) * 100
        message = f"rendering frames: {self.current_frame}/{self.total_frames} ({percent:5.1f}%)"
        if self.is_tty:
            print(f"\r{message}", end="", file=sys.stderr, flush=True)
            if self.current_frame >= self.total_frames:
                print(file=sys.stderr, flush=True)
        else:
            print(message, file=sys.stderr, flush=True)

        self.last_reported_frame = self.current_frame


def render_video(
    steps: list[DemoStep],
    repository_root: Path,
    output_path: Path,
    gif_output_path: Path,
    preflight_signature_value: str,
) -> None:
    font = load_font(FONT_SIZE)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    gif_output_path.parent.mkdir(parents=True, exist_ok=True)

    should_render_assets, signature = should_render(
        steps,
        repository_root,
        output_path,
        gif_output_path,
        preflight_signature_value,
    )
    if not should_render_assets:
        log_status("usage demo assets are up to date; skipping render")
        return

    embedded_state = expected_embedded_state(
        repository_root,
        gif_output_path,
        preflight_signature=preflight_signature_value,
        render_signature=signature,
    )

    with tempfile.TemporaryDirectory(prefix="transfer-rs-video-frames-") as frames_dir:
        frames_path = Path(frames_dir)
        frame_index = 0
        progress = RenderProgress(total_render_frames(steps))

        def write_repeated(lines: list[str], count: int) -> None:
            nonlocal frame_index
            for _ in range(count):
                frame_file = frames_path / f"frame_{frame_index:05d}.png"
                draw_frame(lines, TITLE, frame_file, font)
                frame_index += 1
                progress.advance()

        write_repeated(INTRO_LINES, FPS * 2)

        transcript: list[str] = []
        for step in steps:
            for character_index in range(1, len(step.display_command) + 1):
                current_lines = transcript + wrap_command(step.display_command[:character_index] + "_")
                write_repeated(current_lines, 1)

            current_lines = transcript + wrap_command(step.display_command)
            write_repeated(current_lines, max(6, FPS // 4))

            output_lines = wrap_output(step.output_lines)
            revealed: list[str] = []
            for line in output_lines:
                revealed.append(line)
                write_repeated(transcript + wrap_command(step.display_command) + revealed, max(7, FPS // 5))

            append_step_lines(transcript, step)
            write_repeated(transcript, FPS // 2)

        write_repeated(transcript + ["", "demo complete"], FPS * 2)

        input_pattern = str(frames_path / "frame_%05d.png")
        palette_path = frames_path / "palette.png"
        candidate_output_path = frames_path / output_path.name
        candidate_gif_output_path = frames_path / gif_output_path.name

        log_status("encoding mp4")
        run_ffmpeg(
            [
                "ffmpeg",
                "-y",
                "-framerate",
                str(FPS),
                "-i",
                input_pattern,
                "-metadata",
                f"comment={embedded_state_text(embedded_state)}",
                "-vf",
                "format=yuv420p",
                str(candidate_output_path),
            ],
        )

        log_status("encoding gif palette")
        run_ffmpeg(
            [
                "ffmpeg",
                "-y",
                "-framerate",
                str(FPS),
                "-i",
                input_pattern,
                "-vf",
                f"fps=15,scale={WIDTH}:-1:flags=lanczos,palettegen",
                str(palette_path),
            ]
        )

        log_status("encoding gif")
        run_ffmpeg(
            [
                "ffmpeg",
                "-y",
                "-framerate",
                str(FPS),
                "-i",
                input_pattern,
                "-i",
                str(palette_path),
                "-lavfi",
                f"fps=15,scale={WIDTH}:-1:flags=lanczos[x];[x][1:v]paletteuse",
                str(candidate_gif_output_path),
            ]
        )
        embed_gif_state(candidate_gif_output_path, embedded_state)

        log_status("updating output files")
        replace_if_changed(candidate_output_path, output_path)
        replace_if_changed(candidate_gif_output_path, gif_output_path)


def main() -> None:
    args = parse_args()
    repository_root = repo_root()
    output_path = repository_root / args.output
    gif_output_path = repository_root / args.gif_output if args.gif_output else output_path.with_suffix(".gif")

    ensure_tool("ffprobe")
    preflight_signature_value = preflight_signature(repository_root)

    log_status("checking demo inputs")
    if not should_regenerate_demo(repository_root, output_path, gif_output_path, preflight_signature_value):
        log_status("usage demo inputs unchanged; skipping demo step generation")
        print(output_path.relative_to(repository_root))
        print(gif_output_path.relative_to(repository_root))
        return

    log_status("generating demo transcript")
    steps = create_demo_steps(repository_root)
    render_video(steps, repository_root, output_path, gif_output_path, preflight_signature_value)
    print(output_path.relative_to(repository_root))
    print(gif_output_path.relative_to(repository_root))


if __name__ == "__main__":
    main()