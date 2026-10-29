#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""公式内存和应用字体检查器的拒绝路径。"""
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import tempfile
import re
import unittest
from unittest.mock import patch


def load(name):
    spec = spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


MATH = load('check-math-view')
PUBLIC = load('check-public-tree')


class MathCheckTests(unittest.TestCase):
    def test_font_filename_list_matches_runtime_registration(self):
        source = (Path(__file__).resolve().parents[1] / 'apps/macos/Sources/Hortus/MathRenderer.swift').read_text()
        filenames = re.findall(r'"(KaTeX_[^"]+\.ttf)"', source)
        self.assertEqual(set(filenames), PUBLIC.KATEX_FONT_FILES)

    def sample(self):
        return {'memory_bytes': [100_000_000] * 20, 'peak_bytes': [110_000_000] * 20,
                'footprint_bytes': [90_000_000] * 20, 'retained_old_views': [1] * 20,
                'retained_old_math_views': [0] * 20}

    def test_stable_single_shell_passes(self):
        MATH.check_memory(self.sample())

    def test_multiple_old_shells_are_rejected(self):
        sample = self.sample()
        sample['retained_old_views'][-1] = 2
        with self.assertRaises(AssertionError):
            MATH.check_memory(sample)

    def test_late_shell_growth_is_rejected(self):
        sample = self.sample()
        sample['retained_old_views'] = [0] * 19 + [1]
        with self.assertRaises(AssertionError):
            MATH.check_memory(sample)

    def test_slow_footprint_growth_is_rejected_even_with_flat_rss_and_peak(self):
        sample = self.sample()
        sample['footprint_bytes'] = [90_000_000 + index * 4096 for index in range(20)]
        with self.assertRaises(AssertionError):
            MATH.check_memory(sample)

    def test_paused_one_mib_per_round_leak_is_rejected(self):
        for key in ['memory_bytes', 'footprint_bytes']:
            with self.subTest(key=key):
                sample = self.sample()
                sample[key] = [90_000_000 + (index - int(index >= 15)) * 1024 * 1024
                               for index in range(20)]
                with self.assertRaises(AssertionError):
                    MATH.check_memory(sample)

    def test_large_footprint_range_is_rejected(self):
        sample = self.sample()
        sample['footprint_bytes'][-1] += 17 * 1024 * 1024
        with self.assertRaises(AssertionError):
            MATH.check_memory(sample)

    def test_twenty_fonts_with_a_wrong_filename_are_rejected(self):
        with tempfile.TemporaryDirectory(prefix='hortus-font-check-') as temporary:
            bundle = Path(temporary)
            resources = bundle / 'Contents/Resources'
            fonts = resources / 'Fonts/KaTeX'
            fonts.mkdir(parents=True)
            binary = bundle / 'Contents/MacOS/Hortus'
            binary.parent.mkdir()
            binary.write_bytes(b'native')
            (resources / 'LICENSE.txt').touch()
            (resources / 'ThirdPartyNotices.txt').write_text(
                'swiftterm uniffi pulldown-cmark Rust standard library ratex-layout KaTeX fonts SIL OPEN FONT LICENSE')
            (fonts / 'OFL.txt').touch()
            for font in (Path(__file__).resolve().parents[1] / 'apps/macos/Resources/Fonts/KaTeX').glob('*.ttf'):
                (fonts / font.name).touch()
            with patch.object(PUBLIC.subprocess, 'run') as run:
                run.return_value.stdout = 'native\n'
                self.assertEqual(PUBLIC.check_bundle(bundle), [])
                (fonts / 'KaTeX_Main-Regular.ttf').rename(fonts / 'KaTeX_Unexpected.ttf')
                self.assertTrue(PUBLIC.check_bundle(bundle))


if __name__ == '__main__':
    unittest.main()
