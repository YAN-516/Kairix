#!/usr/bin/env python3
import argparse
import pathlib
import shutil
import subprocess
import tempfile


SECTOR_SIZE = 512
DEFAULT_CHUNK_SIZE = 64 * 1024 * 1024
DEFAULT_PART_SIZE = 2 * 1024 * 1024 * 1024
COMMANDS_PER_BATCH = 8


def hex_value(value: int) -> str:
    return f"0x{value:x}"


def build_commands(image_name: str, image_size: int, chunk_size: int, part_size: int) -> str:
    if image_size == 0 or image_size % SECTOR_SIZE != 0:
        raise ValueError("rootfs image size must be a non-zero multiple of 512 bytes")
    if chunk_size == 0 or chunk_size % SECTOR_SIZE != 0:
        raise ValueError("chunk size must be a non-zero multiple of 512 bytes")
    if part_size == 0 or part_size % chunk_size != 0:
        raise ValueError("part size must be a non-zero multiple of the chunk size")

    total_sectors = image_size // SECTOR_SIZE
    part_count = (image_size + part_size - 1) // part_size
    source = image_name if part_count == 1 else f"{image_name}.part00..part{part_count - 1:02d}"
    lines = [
        "echo Kairix SATA rootfs writer",
        f"echo Source: USB {source}",
        f"echo Image bytes: {image_size}",
        f"echo Target sectors: {hex_value(total_sectors)}",
        "echo WARNING: SCSI device 0 will be overwritten",
        "usb start",
        "scsi scan",
        "scsi device 0",
    ]

    chunks = (image_size + chunk_size - 1) // chunk_size
    for batch_start in range(0, chunks, COMMANDS_PER_BATCH):
        batch_end = min(batch_start + COMMANDS_PER_BATCH, chunks)
        run_names = []
        for chunk_index in range(batch_start, batch_end):
            slot = chunk_index - batch_start
            offset = chunk_index * chunk_size
            byte_count = min(chunk_size, image_size - offset)
            lba = offset // SECTOR_SIZE
            sector_count = byte_count // SECTOR_SIZE
            part_index = offset // part_size
            part_offset = offset % part_size
            usb_name = image_name if part_count == 1 else f"{image_name}.part{part_index:02d}"
            lines.append(
                f"setenv load{slot} fatload usb 0:1 ${{loadaddr}} {usb_name} "
                f"{hex_value(byte_count)} {hex_value(part_offset)}"
            )
            lines.append(
                f"setenv write{slot} scsi write ${{loadaddr}} "
                f"{hex_value(lba)} {hex_value(sector_count)}"
            )
            run_names.extend((f"load{slot}", f"write{slot}"))

        lines.append("setenv sata_batch run " + " ".join(run_names))
        lines.append("run sata_batch")
        lines.append(f"echo SATA progress: {batch_end}/{chunks} chunks")

    lines.extend(
        [
            "echo SATA ROOTFS WRITE COMPLETE",
            "echo Reset the board and boot from the USB menu",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate a loop-free U-Boot script that writes an ext4 image to SCSI disk 0."
    )
    parser.add_argument("--image", required=True, help="local rootfs image")
    parser.add_argument(
        "--usb-path", default="/install/sata-rootfs.img", help="image path on the USB FAT partition"
    )
    parser.add_argument("--out", required=True, help="output legacy U-Boot script image")
    parser.add_argument("--command-out", help="optional output path for the generated text commands")
    parser.add_argument("--chunk-size", type=lambda value: int(value, 0), default=DEFAULT_CHUNK_SIZE)
    parser.add_argument("--part-size", type=lambda value: int(value, 0), default=DEFAULT_PART_SIZE)
    args = parser.parse_args()

    image = pathlib.Path(args.image)
    if not image.is_file():
        raise SystemExit(f"rootfs image not found: {image}")
    mkimage = shutil.which("mkimage")
    if mkimage is None:
        raise SystemExit("mkimage is required (install u-boot-tools)")

    try:
        commands = build_commands(
            args.usb_path, image.stat().st_size, args.chunk_size, args.part_size
        )
    except ValueError as error:
        raise SystemExit(str(error)) from error

    if args.command_out:
        pathlib.Path(args.command_out).write_text(commands, encoding="ascii")

    output = pathlib.Path(args.out)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="ascii") as command_file:
        command_file.write(commands)
        command_file.flush()
        subprocess.run(
            [
                mkimage,
                "-A",
                "mips64",
                "-O",
                "linux",
                "-T",
                "script",
                "-C",
                "none",
                "-n",
                "Kairix SATA rootfs writer",
                "-d",
                command_file.name,
                str(output),
            ],
            check=True,
        )

    chunks = (image.stat().st_size + args.chunk_size - 1) // args.chunk_size
    parts = (image.stat().st_size + args.part_size - 1) // args.part_size
    print(
        f"generated {output}: {image.stat().st_size} bytes, {parts} FAT files, {chunks} chunks"
    )


if __name__ == "__main__":
    main()
