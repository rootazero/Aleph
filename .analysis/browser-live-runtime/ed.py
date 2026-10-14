import sys,json
def apply(path, edits):
    raw=open(path,'rb').read().decode('utf8')
    crlf='\r\n' in raw
    t=raw.replace('\r\n','\n')
    for old,new in edits:
        assert t.count(old)==1, (path, t.count(old), old[:80])
        t=t.replace(old,new)
    if crlf: t=t.replace('\n','\r\n')
    open(path,'wb').write(t.encode('utf8'))
