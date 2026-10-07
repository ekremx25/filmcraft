#!/usr/bin/env python3
"""Launch a real AppImage and export H.264 through its desktop control API.

Requires an active desktop, AMD GPU, amdgpu_top, and ffmpeg/ffprobe test oracles.
Only the test's own application/monitor processes are stopped. Evidence is kept.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('appimage', type=Path)
    parser.add_argument('--seconds', type=int, default=15)
    args = parser.parse_args()
    if not 5 <= args.seconds <= 60:
        parser.error('--seconds must be 5..60')
    image = args.appimage.resolve(strict=True)
    if not (os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')):
        parser.error('An active desktop session is required')
    for tool in ('amdgpu_top', 'ffmpeg', 'ffprobe'):
        if not shutil.which(tool):
            parser.error(f'{tool} is required for this hardware test')
    root = Path(__file__).resolve().parents[1]
    out = root/'target'/f'appimage-vaapi-test-{time.time_ns()}'
    out.mkdir(parents=True)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]

    def call(method, params=None):
        with socket.create_connection(('127.0.0.1', port), timeout=10) as sock:
            sock.settimeout(20)
            sock.sendall((json.dumps({'id': 1, 'method': method, 'params': params or {}})+'\n').encode())
            reply = json.loads(sock.makefile().readline())
        if not reply.get('ok'):
            raise RuntimeError(reply)
        return reply.get('result')

    def execute(command, params=None):
        return call('engine.execute', {'command': command, 'params': params or {}})

    env = dict(os.environ, FILMCRAFT_LOG='info')
    # Default launch exercises FUSE. User may explicitly choose extract-and-run.
    report = {'appimage': str(image), 'size_bytes': image.stat().st_size,
              'sha256': hashlib.sha256(image.read_bytes()).hexdigest(),
              'launch': 'FAIL', 'vaapi_detection': 'NOT RUN', 'vcn_activity': 'NOT RUN',
              'evidence': str(out)}
    proc = monitor = None
    app_log = (out/'application.log').open('w')
    monitor_log = (out/'amdgpu-top.json').open('w')
    tracked_pids = set()
    mapped = set()
    drm = set()
    try:
        proc = subprocess.Popen([str(image), '--demo', '--control', str(port), '--no-recover',
                                 '--data-dir', str(out/'profile')], env=env, stdout=app_log,
                                stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic()+60
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError('AppImage exited before launch; inspect application.log')
            try:
                state = call('ui.inspect')
                break
            except (OSError, ValueError):
                time.sleep(0.5)
        else:
            raise RuntimeError('Desktop control server did not start')
        report['launch'] = 'PASS'
        (out/'ui-state.json').write_text(json.dumps(state, indent=2))
        report['version'] = subprocess.check_output([str(image), '--version'], env=env, text=True).strip()
        monitor = subprocess.Popen(['amdgpu_top', '-J', '-u', '1', '-s', '100'], stdout=monitor_log,
                                   stderr=subprocess.DEVNULL)
        movie = out/'h264-hardware.mp4'
        result = execute('file.exportMedia', {
            'path': str(movie), 'format': 'h264', 'width': 1920, 'height': 1080, 'fps': 30,
            'range': 'custom', 'startSeconds': 0, 'endSeconds': args.seconds, 'audio': False,
            'settings': {'videoEncoding': 'hardware', 'bitrateMode': 'vbr1Pass', 'bitrateKbps': 20000}})
        (out/'export-start.json').write_text(json.dumps(result, indent=2))
        deadline = time.monotonic()+300
        while time.monotonic() < deadline:
            # Capture only this process group; don't confuse another FilmCraft session's GPU work.
            for path in Path('/proc').iterdir():
                if not path.name.isdigit():
                    continue
                try:
                    pid = int(path.name)
                    if os.getpgid(pid) != proc.pid:
                        continue
                    exe = os.readlink(path/'exe')
                    if not exe.endswith('/filmcraft'):
                        continue
                    tracked_pids.add(str(pid))
                    report['mounted_executable'] = exe
                    for line in (path/'maps').read_text().splitlines():
                        if any(k in line for k in ('libva', '_drv_video', 'libvulkan', 'libdrm', 'libgallium', 'libEGL', 'libGL', 'libgbm')):
                            mapped.add(line.split()[-1])
                    for fd in (path/'fd').iterdir():
                        try:
                            name = os.readlink(fd)
                            if name.startswith('/dev/dri/'):
                                drm.add(name)
                        except OSError:
                            pass
                except (OSError, ProcessLookupError):
                    continue
            jobs = execute('jobs.list')
            if jobs and all(j.get('finished') for j in jobs):
                (out/'jobs.json').write_text(json.dumps(jobs, indent=2))
                if any((j.get('result') or {}).get('error') for j in jobs):
                    raise RuntimeError(jobs)
                break
            time.sleep(0.25)
        else:
            raise RuntimeError('Hardware export timed out')
        call('ui.set', {'mode': 'export', 'export': {'settings': {'videoEncoding': 'hardware'}, 'location': str(out), 'fileName': 'h264-hardware'}})
        call('ui.screenshot', {'path': str(out/'appimage-export.png')})
        monitor.terminate()
        monitor.wait(timeout=10)
        monitor_log.flush()
        text = (out/'amdgpu-top.json').read_text()
        decoder = json.JSONDecoder()
        values = []
        while text.strip():
            try:
                sample, consumed = decoder.raw_decode(text.lstrip())
            except json.JSONDecodeError:
                break  # Monitor termination can interrupt its final JSON sample.
            text = text.lstrip()[consumed:]
            for device in sample.get('devices', []):
                for pid, record in device.get('fdinfo', {}).items():
                    if pid in tracked_pids:
                        use = record.get('usage', {}).get('usage', {})
                        values.append(max((use.get(k) or {}).get('value', 0) for k in ('VCN_Unified', 'Encode', 'VCN_Encode')))
        report['vcn_peak_percent'] = max(values, default=0)
        report['vcn_process_samples'] = len(values)
        report['render_nodes'] = sorted(drm)
        report['host_driver_libraries'] = sorted(mapped)
        log = (out/'application.log').read_text()
        report['encoder_log'] = [line for line in log.splitlines() if 'FilmCraft export encoder: VAAPI' in line]
        if not report['encoder_log'] or not any('renderD' in d for d in drm):
            raise RuntimeError('VAAPI encoder / render-node access was not confirmed')
        if any('/tmp/.mount_' in lib or 'squashfs-root' in lib for lib in mapped):
            raise RuntimeError('A graphics library was loaded from the AppImage')
        report['vaapi_detection'] = 'PASS'
        if report['vcn_peak_percent'] <= 0:
            raise RuntimeError('amdgpu_top did not observe this process using VCN')
        report['vcn_activity'] = 'PASS'
        probe = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-count_frames',
            '-show_entries', 'stream=codec_name,width,height,nb_read_frames', '-of', 'json', str(movie)], text=True))
        video = probe['streams'][0]
        if video['codec_name'] != 'h264' or int(video['nb_read_frames']) != args.seconds*30:
            raise RuntimeError(probe)
        subprocess.run(['ffmpeg', '-v', 'error', '-xerror', '-i', str(movie), '-f', 'null', '-'], check=True)
        report['export_decode'] = 'PASS'
        report['video'] = video
    finally:
        if monitor and monitor.poll() is None:
            monitor.terminate()
            monitor.wait(timeout=10)
        if proc and proc.poll() is None:
            try:
                call('app.quit')
                proc.wait(timeout=15)
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait(timeout=10)
        app_log.close()
        monitor_log.close()
        (out/'report.json').write_text(json.dumps(report, indent=2)+'\n')
        print(json.dumps(report, indent=2), flush=True)


if __name__ == '__main__':
    main()
