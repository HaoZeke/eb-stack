import os, shutil
_rmtree = shutil.rmtree
_setitem = os.environ.__class__.__setitem__
def _on():
    return os.environ.get("EB_STACK_KEEP_SESSION_TMP") == "1"
def _session(path):
    tmp = os.environ.get("TMPDIR")
    if not _on() or not tmp or path is None:
        return False
    tmp_abs = os.path.abspath(tmp)
    return os.path.abspath(str(path)) == tmp_abs and os.path.basename(tmp_abs).startswith("eb-")
def rmtree(path, *args, **kwargs):
    return None if _session(path) else _rmtree(path, *args, **kwargs)
def setitem(self, key, value):
    if key == "TMPDIR" and _on() and not isinstance(value, str):
        return None
    return _setitem(self, key, value)
shutil.rmtree = rmtree
os.environ.__class__.__setitem__ = setitem
