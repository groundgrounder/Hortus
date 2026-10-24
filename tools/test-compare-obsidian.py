#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Regression checks for measurement validity; no GUI or installed apps needed."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location('comparison', Path(__file__).with_name('compare-obsidian.py'))
MEASURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MEASURE)


class MeasurementValidity(unittest.TestCase):
    def test_footprint_uses_current_value_and_never_peak_or_arbitrary_number(self):
        self.assertEqual(MEASURE.parse_footprint('phys_footprint: 135,742,520 B\nphys_footprint_peak: 217859128 B\n'), 135742520)
        self.assertIsNone(MEASURE.parse_footprint('phys_footprint_peak: 217859128 B\npage count 1234'))
        self.assertIsNone(MEASURE.parse_footprint('phys_footprint: 10 B\nphys_footprint: 20 B\n'))

    def test_partial_footprint_cannot_be_reported_as_the_total(self):
        outputs = [subprocess.CompletedProcess([], 0, 'phys_footprint: 100 B', ''),
                   subprocess.CompletedProcess([], 1, '', 'permission denied')]
        with tempfile.TemporaryDirectory() as directory, patch.object(MEASURE, 'command', side_effect=outputs):
            value, method, errors = MEASURE.footprint_bytes([10, 11], Path(directory))
        self.assertIsNone(value)
        self.assertEqual(method, 'unavailable')
        self.assertTrue(errors)

    def test_wrong_note_and_failed_samples_are_excluded_from_median(self):
        result = MEASURE.summarize([{'elapsed_ms': 10, 'title_match': False},
                                    {'elapsed_ms': 100, 'title_match': True},
                                    {'elapsed_ms': 120, 'title_match': True},
                                    {'elapsed_ms': None, 'title_match': False}], 'elapsed_ms')
        self.assertEqual(result, {'samples': [100, 120], 'median': 110, 'count': 2, 'attempts': 4})

    def test_process_and_network_inspection_failures_are_not_zero(self):
        failure = subprocess.CompletedProcess([], 126, '', 'Operation not permitted')
        with patch.object(MEASURE, 'command', return_value=failure):
            table, error = MEASURE.process_table()
            connections = MEASURE.lsof_connections([10])
        self.assertEqual(table, {})
        self.assertTrue(error)
        self.assertFalse(connections['available'])
        self.assertFalse(MEASURE.lsof_connections([])['available'])

    def test_only_launched_process_descendants_are_selected(self):
        table = {10: {'ppid': 1}, 11: {'ppid': 10}, 12: {'ppid': 11}, 20: {'ppid': 1}}
        self.assertEqual(MEASURE.descendants(table, [10]), [10, 11, 12])
        self.assertEqual(MEASURE.descendants(table, [99]), [])

    def test_network_monitor_stops_after_process_exit_without_recording_zero(self):
        for statuses, snapshot in (([0], {}), ([None, 0], {})):
            process = SimpleNamespace(pid=10, poll=Mock(side_effect=statuses))
            with patch.object(MEASURE, 'process_table', return_value=(snapshot, None)), \
                    patch.object(MEASURE, 'lsof_connections') as inspect:
                monitor = MEASURE.NetworkMonitor(process)
                monitor.thread.join(timeout=1)
                self.assertFalse(monitor.thread.is_alive())
                self.assertEqual(monitor.samples, [])
                inspect.assert_not_called()

    def test_source_inventory_detects_additions_deletions_and_edits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            note = root / 'note.md'
            note.write_text('original')
            before = MEASURE.inventory(root)
            note.write_text('edited')
            self.assertNotEqual(before, MEASURE.inventory(root))
            note.write_text('original')
            self.assertEqual(before, MEASURE.inventory(root))
            (root / 'unexpected').write_text('state')
            self.assertNotEqual(before, MEASURE.inventory(root))
            note.unlink()
            self.assertNotEqual(before, MEASURE.inventory(root))

    def test_generated_note_is_reproducible_utf8_with_public_license(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            first = MEASURE.make_large_note(Path(a))
            second = MEASURE.make_large_note(Path(b))
            self.assertEqual(first, second)
            data = (Path(a) / first['path']).read_bytes()
            self.assertGreaterEqual(len(data), 1024**2)
            self.assertLess(len(data), 1024**2 + 100)
            self.assertIn('SPDX-License-Identifier: CC0-1.0', data.decode())
            self.assertIn('Paragraph 8000:', data.decode())


if __name__ == '__main__':
    unittest.main()
