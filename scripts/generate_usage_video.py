#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
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


def create_demo_steps(repository_root: Path) -> list[DemoStep]:
    ensure_tool("cargo")
    ensure_tool("ffmpeg")

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

        server = DemoServer(("127.0.0.1", 0))
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()

        try:
            help_result = run_command([str(binary), "--help"], cwd=work_path, env=env)
            help_lines = help_result.stdout.strip().splitlines()
            if len(help_lines) > 13:
                help_lines = help_lines[:12] + ["..."]

            upload_result = run_command(
                [str(binary), "--server", server.base_url, "upload", "source.txt"],
                cwd=work_path,
                env=env,
            )

            source_file.unlink()

            download_url = f"{server.base_url}/source.txt"
            download_result = run_command([str(binary), "download", download_url], cwd=work_path, env=env)
            cat_result = run_command(["cat", "source.txt"], cwd=work_path, env=env)
            delete_result = run_command([str(binary), "delete", download_url], cwd=work_path, env=env)
        finally:
            server.shutdown()
            server.server_close()
            server_thread.join(timeout=2)

    return [
        DemoStep("transfer-rs --help", help_lines),
        DemoStep(
            f"transfer-rs --server {server.base_url} upload source.txt",
            upload_result.stdout.strip().splitlines(),
        ),
        DemoStep("rm source.txt", []),
        DemoStep(f"transfer-rs download {download_url}", download_result.stdout.strip().splitlines()),
        DemoStep("cat source.txt", cat_result.stdout.strip().splitlines()),
        DemoStep(f"transfer-rs delete {download_url}", delete_result.stdout.strip().splitlines()),
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


def render_video(steps: list[DemoStep], output_path: Path, gif_output_path: Path) -> None:
    font = load_font(FONT_SIZE)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    gif_output_path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="transfer-rs-video-frames-") as frames_dir:
        frames_path = Path(frames_dir)
        frame_index = 0

        def write_repeated(lines: list[str], count: int) -> None:
            nonlocal frame_index
            for _ in range(count):
                frame_file = frames_path / f"frame_{frame_index:05d}.png"
                draw_frame(lines, "transfer-rs usage demo", frame_file, font)
                frame_index += 1

        write_repeated([
            "transfer-rs usage demo",
            "upload, download, and delete against a local transfer-compatible server",
            "",
            "generated from real CLI output",
        ], FPS * 2)

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

        run_ffmpeg(
            [
                "ffmpeg",
                "-y",
                "-framerate",
                str(FPS),
                "-i",
                input_pattern,
                "-vf",
                "format=yuv420p",
                str(output_path),
            ],
        )

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
                str(gif_output_path),
            ]
        )


def main() -> None:
    args = parse_args()
    repository_root = repo_root()
    output_path = repository_root / args.output
    gif_output_path = repository_root / args.gif_output if args.gif_output else output_path.with_suffix(".gif")
    steps = create_demo_steps(repository_root)
    render_video(steps, output_path, gif_output_path)
    print(output_path.relative_to(repository_root))
    print(gif_output_path.relative_to(repository_root))


if __name__ == "__main__":
    main()