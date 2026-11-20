# Windows placeholder for the Unix module pwd (2.x test fixture only, see grp.py).
def getpwuid(uid):
    raise KeyError(uid)
