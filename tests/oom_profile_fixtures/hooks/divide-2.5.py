# integers.py with the libxc divider the maintainers suggested in
# EESSI/software-layer#1772 (issuecomment-6420263033): 2.5 instead of 2.
# 16 // 2.5 is 6.0 in Python, a float.
# stack oom --hooks parses this file and never runs it.

CPU_TARGET_A64FX = 'aarch64/a64fx'


def divide_by_factor(parallel, factor):
    return max(1, parallel // factor)


def set_maximum(parallel, max_value):
    return min(parallel, max_value)


PARALLELISM_LIMITS = {
    CPU_TARGET_A64FX: (divide_by_factor, 4),
    'libxc': {
        '*': (divide_by_factor, 2.5),
        CPU_TARGET_A64FX: (set_maximum, 6),
    },
}
