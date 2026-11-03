# msg-plus.py with the log message appended by msg += "...".
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


def post_ready_hook(self, *args, **kwargs):
    new_parallel = 8
    msg = "limiting parallelism to %s for %s "
    msg += "to avoid out-of-memory failures during building/testing"
    print(msg % (new_parallel, self.name))
