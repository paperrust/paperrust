# Windows placeholder for the Unix module grp (only for the 2.x test fixture; paperless-ngx 2.x
# imports it in paperless/checks.py, Django migrations do not need it).
def getgrgid(gid):
    raise KeyError(gid)
