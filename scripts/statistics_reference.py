#!/usr/bin/env python3
"""Independent high-precision reference for v4.1 E01/V085.

This verifies program mathematics only. It is not product-effect evidence.
"""

from __future__ import annotations

import json
import math
import random
from decimal import Decimal, getcontext

getcontext().prec = 80

ALPHA = Decimal("0.01")
RHO = Decimal(1)
MIN_GAIN = Decimal("0.02")
MAX_REGRESSION = Decimal("0.01")
OUTWARD_ERROR = Decimal("1e-12")


def reference_boundary(k: int, sum_micros: int) -> tuple[Decimal, Decimal, Decimal, Decimal]:
    if k <= 0:
        raise ValueError("k must be positive")
    kd = Decimal(k)
    mean = Decimal(sum_micros) / (kd * Decimal(1_000_000))
    log_term = (kd + RHO).ln() - RHO.ln() - Decimal(2) * ALPHA.ln()
    radius = ((kd + RHO) * log_term).sqrt() / kd
    return mean, radius, max(Decimal(-1), mean - radius), min(Decimal(1), mean + radius)


def implementation_boundary(
    k: int, sum_micros: int, outward_error: float = 0.0
) -> tuple[float, float]:
    mean = sum_micros / (k * 1_000_000.0)
    log_term = math.log(k + 1.0) - 2.0 * math.log(0.01)
    radius = math.sqrt((k + 1.0) * log_term) / k + outward_error
    return max(-1.0, mean - radius), min(1.0, mean + radius)


def first_stop(values_micros: list[int]) -> tuple[int | None, str | None]:
    total = 0
    for k, value in enumerate(values_micros, start=1):
        total += value
        _, _, _, ucb = reference_boundary(k, total)
        if ucb < -MAX_REGRESSION:
            return k, "sequential_regression"
        if ucb < MIN_GAIN:
            return k, "futility_quality_gain"
    return None, None


def main() -> None:
    golden: dict[str, str] = {}
    for k in (12, 13, 14):
        _, _, _, ucb = reference_boundary(k, -1_000_000 * k)
        golden[f"k{k}_ucb"] = format(ucb, ".16f")

    assert abs(Decimal(golden["k12_ucb"]) - Decimal("0.0310417011")) < Decimal("1e-10")
    assert abs(Decimal(golden["k13_ucb"]) - Decimal("-0.0092392266")) < Decimal("1e-10")
    assert abs(Decimal(golden["k14_ucb"]) - Decimal("-0.0449493587")) < Decimal("1e-10")
    assert first_stop([-1_000_000] * 60) == (13, "futility_quality_gain")

    max_float_error = Decimal(0)
    # Scan the supported k range and representative signed-micros means. The
    # Rust boundary widens by 1e-12; fail if the observed reference error is
    # not strictly inside that envelope.
    for k in list(range(1, 1001)) + [2_000, 5_000, 10_000, 50_000, 100_000]:
        for per_unit in (-1_000_000, -20_001, -20_000, -10_001, -10_000, 0, 20_000, 1_000_000):
            _, _, ref_lcb, ref_ucb = reference_boundary(k, per_unit * k)
            raw_lcb, raw_ucb = implementation_boundary(k, per_unit * k)
            f_lcb, f_ucb = implementation_boundary(k, per_unit * k, 1e-12)
            max_float_error = max(
                max_float_error,
                abs(Decimal.from_float(raw_lcb) - ref_lcb),
                abs(Decimal.from_float(raw_ucb) - ref_ucb),
            )
            assert Decimal.from_float(f_lcb) <= ref_lcb
            assert Decimal.from_float(f_ucb) >= ref_ucb

    rng = random.Random(0xE01_4085)
    null_false_regressions = 0
    degradation_stops: list[int] = []
    simulations = 2_000
    for _ in range(simulations):
        null = [1_000_000 if rng.getrandbits(1) else -1_000_000 for _ in range(60)]
        stop, reason = first_stop(null)
        null_false_regressions += int(reason == "sequential_regression")

        degraded = [-1_000_000 if rng.random() < 0.8 else 0 for _ in range(60)]
        stop, _ = first_stop(degraded)
        if stop is not None:
            degradation_stops.append(stop)

    # Loose deterministic program guards. These do not estimate project gain.
    assert null_false_regressions <= 40
    assert len(degradation_stops) >= 1_900

    print(
        json.dumps(
            {
                "schema": "rsia.statistics_reference.v1",
                "golden": golden,
                "max_float_error": str(max_float_error),
                "numeric_outward_error": str(OUTWARD_ERROR),
                "seed": "0xE014085",
                "simulations": simulations,
                "null_false_regressions": null_false_regressions,
                "degradation_stopped": len(degradation_stops),
                "scope": "program_check_not_product_effect",
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
