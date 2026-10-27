#!/usr/bin/env python3
"""Opt-in terminal-input probe. No Kiro access, files, subprocesses or input logging."""
import argparse
from collections import Counter
import os
import re
import select
import signal
import sys
import termios
import time
import tty

CSI = re.compile(rb'\x1b\[([0-?]*)([ -/]*)([@-~])')
MOUSE = re.compile(rb'<(\d+);\d+;\d+')
OFF = b'\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l'
ALT_OFF = b'\x1b[?1049l'


def classify(data):
    """Discard text, including pasted secrets; report recognized control events only."""
    events = []
    action = None
    while data:
        if data.startswith(b'\x1b[M'):
            if len(data) < 6:
                break
            button = (data[3] - 32) & 0x43
            if button in (64, 65):
                events.append('X10-wheel-up' if button == 64 else 'X10-wheel-down')
            data = data[6:]
        elif data.startswith(b'\x1bO'):
            if len(data) < 3:
                break
            if data[2:3] in (b'A', b'B'):
                events.append('SS3-arrow-up' if data[2:3] == b'A' else 'SS3-arrow-down')
            data = data[3:]
        elif data.startswith(b'\x1b['):
            match = CSI.match(data)
            if not match:
                if len(data) < 64:
                    break
                data = data[1:]
                continue
            args, _, final = match.groups()
            mouse = MOUSE.fullmatch(args)
            if mouse and final == b'M':
                button = int(mouse[1]) & 0x43
                if button in (64, 65):
                    events.append('SGR-wheel-up' if button == 64 else 'SGR-wheel-down')
            elif final in (b'A', b'B'):
                events.append('CSI-arrow-up' if final == b'A' else 'CSI-arrow-down')
            elif final == b'~' and args in (b'5', b'6'):
                events.append('PageUp' if args == b'5' else 'PageDown')
            data = data[match.end():]
        elif data == b'\x1b':
            break
        else:
            if data[:1] in (b'q', b'Q', b'\x03', b'\x04'):
                action = 'quit'
            elif data[:1] in (b'\r', b'\n'):
                action = 'next'
            data = data[1:]
    return data, events, action


def write(text):
    os.write(1, text.encode().replace(b'\n', b'\r\n'))


def run(seconds):
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise SystemExit('Run this in an ordinary interactive MobaXterm SSH shell.')
    if os.environ.get('TMUX'):
        raise SystemExit('Run outside tmux, in a separate ordinary SSH shell; keep Kiro sessions running.')
    saved = termios.tcgetattr(0)
    counts = [Counter(), Counter()]
    phase = 0
    pending = b''

    def stop(*_):
        raise KeyboardInterrupt

    handlers = {sig: signal.signal(sig, stop) for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)}
    try:
        tty.setraw(0)
        os.write(1, OFF + b'\x1b[?1l')
        for number in range(120):
            write(f'Probe sample line {number:03}: harmless terminal scrollback\n')
        write('PHASE 1: normal screen, mouse reporting OFF. Scroll up/down.\n'
              'Press Enter for the tmux-like screen; q quits. Do not type/paste other text.\n')
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not select.select([0], [], [], 0.1)[0]:
                continue
            chunk = os.read(0, 4096)
            if not chunk:
                break
            pending, events, action = classify(pending + chunk)
            counts[phase].update(events)
            for event in events:
                write(f'Observed: {event}\n')
            if action == 'quit':
                break
            if action == 'next' and phase == 0:
                phase = 1
                pending = b''
                os.write(1, b'\x1b[?1049h\x1b[?1h\x1b[?1000h\x1b[?1002h\x1b[?1006h')
                write('PHASE 2: alternate screen, application cursors, mouse ON (tmux-like).\n'
                      'Scroll up/down, then press PageUp/PageDown. q quits.\n')
    except KeyboardInterrupt:
        pass
    finally:
        try:
            os.write(1, OFF + b'\x1b[?1l' + ALT_OFF)
        finally:
            termios.tcsetattr(0, termios.TCSANOW, saved)
            for sig, handler in handlers.items():
                signal.signal(sig, handler)
    print('Recognized control events only; no typed text or files were recorded.')
    for label, count in zip(('normal', 'alternate+mouse'), counts):
        print(label + ': ' + (', '.join(f'{key}={value}' for key, value in sorted(count.items())) or 'no input events'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=45)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        assert classify(b'\x1b[<64;10;5M')[1] == ['SGR-wheel-up']
        assert classify(b'\x1b[M' + bytes([97, 42, 37]))[1] == ['X10-wheel-down']
        assert classify(b'\x1b[A\x1bOB\x1b[5~')[1] == ['CSI-arrow-up', 'SS3-arrow-down', 'PageUp']
        assert classify(b'ignored pasted text')[1] == []
        assert classify(b'\x1b[<64;')[0] == b'\x1b[<64;'
        print('PASS: SGR/X10 wheel, CSI/SS3 arrows, paging, fragmented input and text discard')
    else:
        if not 5 <= args.seconds <= 60:
            parser.error('--seconds must be between 5 and 60')
        run(args.seconds)


if __name__ == '__main__':
    main()
