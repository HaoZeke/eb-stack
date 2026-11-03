# integers.py with libxc's '*' row a reduction of 8, (divide_by_factor, 8),
# as asked in the review of EESSI/software-layer-scripts#337
# (issuecomment-6599147327): 2 cores on a 16-core build node. The A64FX
# row stays at 6.
# stack oom --hooks parses this file and never runs it.

CPU_TARGET_A64FX = 'aarch64/a64fx'


def divide_by_factor(parallel, factor):
    return max(1, parallel // factor)


def set_maximum(parallel, max_value):
    return min(parallel, max_value)


PARALLELISM_LIMITS = {
    CPU_TARGET_A64FX: (divide_by_factor, 4),
    'libxc': {
        '*': (divide_by_factor, 8),
        CPU_TARGET_A64FX: (set_maximum, 6),
    },
}
