#!/usr/bin/env python3
"""Unfunded sample-size estimator for E01. Does not call models or spend money.

Prints a feasible n or INFEASIBLE. Never lowers min_effect.
"""
from __future__ import annotations

import argparse
import math
import sys

RANGE_R = 2.0


def radius(n: int, variance: float, alpha_i: float) -> float:
    ln = math.log(2.0 / alpha_i)
    return math.sqrt(2.0 * variance * ln / n) + 7.0 * RANGE_R * ln / (3.0 * (n - 1))


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--variance", type=float, required=True)
    p.add_argument("--min-effect", type=float, default=0.02)
    p.add_argument("--alpha", type=float, default=0.05)
    p.add_argument("--max-n", type=int, default=10000)
    p.add_argument("--max-cost-units", type=int, default=0)
    p.add_argument("--unit-cost", type=int, default=1)
    args = p.parse_args()
    if args.variance < 0 or args.min_effect <= 0 or args.max_n < 2:
        print("invalid inputs", file=sys.stderr)
        return 2
    if args.max_cost_units == 0:
        print("INFEASIBLE monetary_budget=0; do not lower min_effect")
        return 1
    for n in range(2, args.max_n + 1):
        if args.min_effect > radius(n, args.variance, args.alpha):
            cost = n * args.unit_cost
            if cost > args.max_cost_units:
                print(f"INFEASIBLE n={n} cost={cost} exceeds budget; threshold unchanged")
                return 1
            print(f"FEASIBLE n={n} cost={cost}")
            return 0
    print("INFEASIBLE n_cap reached; threshold unchanged")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
