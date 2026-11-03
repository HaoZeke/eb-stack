# PARALLELISM_LIMITS in the layout of software-layer-scripts eb_hooks.py,
# cut down to the A64FX row and libxc. Every divide_by_factor is an integer.
# stack oom --hooks parses this file and never runs it.

CPU_TARGET_A64FX = 'aarch64/a64fx'


def divide_by_factor(parallel, factor):
    return max(1, parallel // factor)


def set_maximum(parallel, max_value):
    return min(parallel, max_value)


PARALLELISM_LIMITS = {
    CPU_TARGET_A64FX: (divide_by_factor, 4),
    'libxc': {
        '*': (divide_by_factor, 2),
        CPU_TARGET_A64FX: (set_maximum, 6),
    },
}
