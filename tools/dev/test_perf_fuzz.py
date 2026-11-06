#!/usr/bin/python3
"""Regression: skipped/empty performance runs must not claim a passing check."""
import contextlib
import io
from pathlib import Path
import random
import runpy
import unittest

Fuzz = runpy.run_path(str(Path(__file__).with_name('perf-fuzz')))['Fuzz']


class Coverage(unittest.TestCase):
    def test_no_measured_bursts_is_failure_without_opening_a_session(self):
        fuzz = Fuzz.__new__(Fuzz)
        fuzz.rng = random.Random(1)
        fuzz.seed, fuzz.quiet, fuzz.step = 1, True, 0
        fuzz.only = ['resize', 'scroll']
        fuzz.failures, fuzz.history, fuzz.opened = [], [], []
        fuzz.worst = {}
        fuzz.resources = lambda: None
        fuzz.fd_kinds = lambda: {}
        fuzz.windows = lambda: {}
        # Every selected operation returns early without a mapped window.
        # This previously printed "ok" even after all steps were skipped.
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(fuzz.run(8, False))
        self.assertEqual(len(fuzz.failures), 1)
        self.assertIn('no measured bursts', fuzz.failures[0])


if __name__ == '__main__':
    unittest.main()
