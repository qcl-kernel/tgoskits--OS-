#!/usr/bin/env python3
"""Generate dependency-free SVG evidence charts from copied JSON/log data."""
from pathlib import Path
import json,re
ROOT=Path(__file__).resolve().parents[1]; DATA=ROOT/'data'; OUT=ROOT/'figures'; OUT.mkdir(exist_ok=True)
BLUE,ORANGE,GREEN,RED,GRID,TEXT='#2468a8','#e07a2d','#4c956c','#c44536','#d9e1e8','#18324b'
def esc(v): return str(v).replace('&','&amp;').replace('<','&lt;').replace('>','&gt;')
def chart(name,title,body,width=1100,height=620,note=''):
    n=f'<text x="70" y="{height-24}" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14" fill="#52616b">{esc(note)}</text>' if note else ''
    (OUT/f'{name}.svg').write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="white"/><text x="70" y="42" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="24" font-weight="700" fill="{TEXT}">{esc(title)}</text>{body}{n}</svg>',encoding='utf-8')
def axes(w,h,ymax,ylabel):
    l,t,r,b=90,80,w-45,h-80; z=[]
    for i in range(6):
        y=b-i*(b-t)/5; v=ymax*i/5; z.append(f'<line x1="{l}" y1="{y:.1f}" x2="{r}" y2="{y:.1f}" stroke="{GRID}"/><text x="8" y="{y+5:.1f}" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="13" fill="#52616b">{v:.3g}</text>')
    z.append(f'<text x="18" y="{(t+b)/2:.1f}" transform="rotate(-90 18 {(t+b)/2:.1f})" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15" fill="{TEXT}">{esc(ylabel)}</text><line x1="{l}" y1="{b}" x2="{r}" y2="{b}" stroke="#52616b"/><line x1="{l}" y1="{t}" x2="{l}" y2="{b}" stroke="#52616b"/>')
    return ''.join(z)
def task1():
    rows=json.loads((DATA/'task1-archive-summary.json').read_text())['results']; labels=['r1','r2','r3','r4']; p99=[r['jitter_ns']['p99']/1e6 for r in rows]; p999=[r['jitter_ns']['p999']/1e6 for r in rows]; missed=[r['missed'] for r in rows]
    w,h,l,b=1100,620,90,535; ymax=max(p999)*1.08; body=axes(w,h,ymax,'jitter (ms; outlier scale)')
    for i,label in enumerate(labels):
        cx=l+120+i*230
        for value,color,cap,dx in [(p99[i],BLUE,'P99',-34),(p999[i],ORANGE,'P999',34)]:
            hh=value/ymax*455; body+=f'<rect x="{cx+dx-24}" y="{b-hh:.1f}" width="48" height="{hh:.1f}" fill="{color}"/><text x="{cx+dx}" y="{b+24}" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="13">{cap}</text>'
        body+=f'<text x="{cx}" y="{b+48}" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15">{label}</text>'
    body+=f'<rect x="900" y="56" width="14" height="14" fill="{BLUE}"/><text x="920" y="68" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P99</text><rect x="970" y="56" width="14" height="14" fill="{ORANGE}"/><text x="990" y="68" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P999</text>'
    chart('task1_jitter_all_runs','Task 1 StarryOS user-space jitter: all runs',body,note='r2 is retained as an observed outlier and is excluded from normal-run claims.')
    normal=[0,2,3]; ymax=max(p999[i] for i in normal)*1.25; body=axes(1000,570,ymax,'jitter (ms)'); p1=[];p2=[]
    for n,i in enumerate(normal):
        x=180+n*300;p1.append(f'{x},{535-p99[i]/ymax*455:.1f}');p2.append(f'{x},{535-p999[i]/ymax*455:.1f}');body+=f'<text x="{x}" y="555" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15">{labels[i]}</text>'
    body+=f'<polyline points="{" ".join(p1)}" fill="none" stroke="{BLUE}" stroke-width="4"/><polyline points="{" ".join(p2)}" fill="none" stroke="{ORANGE}" stroke-width="4"/>'
    for pts,c in [(p1,BLUE),(p2,ORANGE)]:
        for pt in pts:
            x,y=pt.split(',');body+=f'<circle cx="{x}" cy="{y}" r="6" fill="{c}"/>'
    body+=f'<line x1="700" y1="70" x2="730" y2="70" stroke="{BLUE}" stroke-width="4"/><text x="740" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P99</text><line x1="800" y1="70" x2="830" y2="70" stroke="{ORANGE}" stroke-width="4"/><text x="840" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P999</text>'
    chart('task1_jitter_normal_comparison','Task 1 normal-run comparison: r1, r3, r4',body,1000,570,'Same workload: 5 ms period, 30 minutes, 360,000 samples per run.')
    ymax=max(missed)*1.25;body=axes(1000,570,ymax,'missed samples')
    for i,v in enumerate(missed):
        x=160+i*210;hh=v/ymax*455;c=RED if i==1 else GREEN;body+=f'<rect x="{x-34}" y="{535-hh:.1f}" width="68" height="{hh:.1f}" fill="{c}"/><text x="{x}" y="555" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15">{labels[i]}</text><text x="{x}" y="{525-hh:.1f}" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="13">{v}</text>'
    chart('task1_missed_samples','Task 1 missed samples: 360,000 samples per run',body,1000,570,'Green: normal observed runs. Red: retained anomalous r2.')
def rtthread():
    rows=json.loads((DATA/'rtthread-baseline-summary.json').read_text())['results']; ss=['continuous_1ms','periodic_idle_10ms','periodic_stress_long_10ms']; names=['continuous 1 ms','idle 10 ms','stress 10 ms'];n=[next(r['jitter_ns']['p99'] for r in rows if r['platform']=='native' and r['scenario']==s)/1e6 for s in ss];a=[next(r['jitter_ns']['p99'] for r in rows if r['platform']=='axvisor' and r['scenario']==s)/1e6 for s in ss];ymax=max(n+a)*1.2;body=axes(1100,620,ymax,'P99 jitter (ms)');p1=[];p2=[]
    for i,name in enumerate(names):
        x=180+i*330;p1.append(f'{x},{535-n[i]/ymax*455:.1f}');p2.append(f'{x},{535-a[i]/ymax*455:.1f}');body+=f'<text x="{x}" y="560" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">{name}</text>'
    body+=f'<polyline points="{" ".join(p1)}" fill="none" stroke="{BLUE}" stroke-width="4"/><polyline points="{" ".join(p2)}" fill="none" stroke="{ORANGE}" stroke-width="4"/>'
    for pts,c in [(p1,BLUE),(p2,ORANGE)]:
        for pt in pts:
            x,y=pt.split(',');body+=f'<circle cx="{x}" cy="{y}" r="6" fill="{c}"/>'
    body+=f'<line x1="800" y1="70" x2="830" y2="70" stroke="{BLUE}" stroke-width="4"/><text x="840" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">native</text><line x1="920" y1="70" x2="950" y2="70" stroke="{ORANGE}" stroke-width="4"/><text x="960" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">AxVisor</text>'
    chart('rtthread_native_axvisor_p99','RT-Thread timing baseline: native versus AxVisor',body,note='Same scenario and period are compared; these are periodic-task captures, not IRQ response latency.')
def task2():
    rows=[]
    for path in sorted((DATA/'task2-direct').glob('*.serial.log')):
        line=next(x for x in path.read_text(errors='replace').splitlines() if 'DIRECT_V2_SUMMARY' in x);rows.append({k:float(v) if '.' in v else int(v) for k,v in re.findall(r'(\w+)=([^\s]+)',line)})
    labels=['100','1,000','10,000','2,000,000'];ymax=max(r['throughput_rps'] for r in rows)*1.2;body=axes(1100,620,ymax,'throughput (requests/s)');pts=[]
    for i,(lab,r) in enumerate(zip(labels,rows)):
        x=170+i*280;y=535-r['throughput_rps']/ymax*455;pts.append(f'{x},{y:.1f}');body+=f'<text x="{x}" y="560" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">{lab} rounds</text><text x="{x}" y="{y-12:.1f}" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="13">{r["throughput_rps"]:.2f}</text>'
    body+=f'<polyline points="{" ".join(pts)}" fill="none" stroke="{BLUE}" stroke-width="4"/>'
    for pt in pts:
        x,y=pt.split(',');body+=f'<circle cx="{x}" cy="{y}" r="6" fill="{BLUE}"/>'
    chart('task2_direct_throughput','Task 2 direct TAP/bridge throughput',body,note='StarryOS 10.77.0.2 -> RT-Thread 10.77.0.3:5000; no relay and no hostfwd.')
    ymax=max(r['rtt_us_p999'] for r in rows)/1000*1.2;body=axes(1100,620,ymax,'RTT (ms)')
    for key,c,label in [('rtt_us_p50',BLUE,'P50'),('rtt_us_p95',GREEN,'P95'),('rtt_us_p999',ORANGE,'P999')]:
        pts=[]
        for i,r in enumerate(rows):
            x=170+i*280;v=r[key]/1000;y=535-v/ymax*455;pts.append(f'{x},{y:.1f}')
        body+=f'<polyline points="{" ".join(pts)}" fill="none" stroke="{c}" stroke-width="4"/>'
        for pt in pts:
            x,y=pt.split(',');body+=f'<circle cx="{x}" cy="{y}" r="5" fill="{c}"/>'
    for i,lab in enumerate(labels):body+=f'<text x="{170+i*280}" y="560" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">{lab}</text>'
    body+=f'<line x1="800" y1="70" x2="830" y2="70" stroke="{BLUE}" stroke-width="4"/><text x="840" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P50</text><line x1="900" y1="70" x2="930" y2="70" stroke="{GREEN}" stroke-width="4"/><text x="940" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P95</text><line x1="1000" y1="70" x2="1030" y2="70" stroke="{ORANGE}" stroke-width="4"/><text x="1040" y="75" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="14">P999</text>'
    chart('task2_direct_rtt','Task 2 direct AICtrl/IP RTT percentiles',body,note='Long-run P999 remains visible so tail behavior is not hidden by P50.')
    metrics=['success','failure','timeout','retry','duplicate','old_seq','protocol_error'];r=rows[-1];ymax=max(r['success'],1)*1.1;body=axes(1100,620,ymax,'count');
    for i,m in enumerate(metrics):
        v=r[m];x=135+i*140;hh=v/ymax*455 if v else 0;c=GREEN if m=='success' else RED;body+=f'<rect x="{x-28}" y="{535-hh:.1f}" width="56" height="{hh:.1f}" fill="{c}"/><text x="{x}" y="560" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="12">{m}</text><text x="{x}" y="{max(515-hh,100):.1f}" text-anchor="middle" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="12">{v}</text>'
    chart('task2_longrun_outcomes','Task 2 long-run result: 4,000,000 direct requests',body,note='success=4,000,000; all failure, timeout, retry, duplicate, old-seq and protocol-error counters=0.')
def completion():
    labels=['T1 SMP3/IPI','T1 jitter capture','T1 strict before/after','T2 direct IP','T2 fault recovery','T3 YOLO/control loop'];vals=[1,1,0,1,1,0];body='<line x1="90" y1="535" x2="1055" y2="535" stroke="#52616b"/><line x1="90" y1="80" x2="90" y2="535" stroke="#52616b"/>'
    for i,(lab,v) in enumerate(zip(labels,vals)):
        y=115+i*68;c=GREEN if v else RED;body+=f'<text x="80" y="{y+8}" text-anchor="end" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15">{lab}</text><rect x="110" y="{y-14}" width="{760 if v else 180}" height="28" fill="{c}"/><text x="890" y="{y+6}" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="15">{"已形成证据" if v else "未形成真实闭环证据"}</text>'
    chart('completion_matrix','Competition evidence completion matrix',body,note='任务三未形成真实 YOLO 推理到控制生效的同一次运行证据；任务一严格 before/after 与逐样本 trace 关联也未完成。')
def architecture():
    (OUT/'architecture.svg').write_text('''<svg xmlns="http://www.w3.org/2000/svg" width="1400" height="820" viewBox="0 0 1400 820"><rect width="1400" height="820" fill="#f7f9fc"/><text x="60" y="64" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="32" font-weight="700" fill="#18324b">AxVisor 混合系统作品架构与证据边界</text><rect x="60" y="110" width="1280" height="620" rx="12" fill="#fff" stroke="#b8c5d1" stroke-width="2"/><rect x="100" y="165" width="340" height="210" rx="10" fill="#eaf3fb" stroke="#2468a8" stroke-width="3"/><text x="130" y="210" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="25" font-weight="700">AxVisor host</text><text x="130" y="252" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">4 host CPU · RISC-V QEMU</text><text x="130" y="286" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">SMP / IPI / vPLIC / vCPU</text><text x="130" y="320" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">AxFS root selector · VM manager</text><rect x="530" y="150" width="350" height="245" rx="10" fill="#eef8f1" stroke="#4c956c" stroke-width="3"/><text x="560" y="195" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="25" font-weight="700">StarryOS guest</text><text x="560" y="237" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">3 vCPU · virtio-blk · virtio-net</text><text x="560" y="271" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">AICtrl/IP v2 client</text><text x="560" y="305" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">周期 probe CSV 已有归档</text><text x="560" y="339" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">用户态新闭环仍受 current 阻塞</text><rect x="930" y="150" width="350" height="245" rx="10" fill="#fff4e8" stroke="#e07a2d" stroke-width="3"/><text x="960" y="195" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="25" font-weight="700">RT-Thread guest</text><text x="960" y="237" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">aictrl_server_v2 · UDP/5000</text><text x="960" y="271" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">AICtrl/IP ACK / 状态 / 错误</text><text x="960" y="305" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">direct TAP/bridge 已压测</text><text x="960" y="339" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">故障恢复与重启恢复已验证</text><line x1="440" y1="265" x2="530" y2="265" stroke="#52616b" stroke-width="4"/><line x1="880" y1="265" x2="930" y2="265" stroke="#52616b" stroke-width="4"/><text x="534" y="450" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="21" fill="#4c956c">virtio-blk / guest rootfs</text><text x="920" y="450" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="21" fill="#e07a2d">virtio-net / AICtrl-IP</text><rect x="145" y="510" width="1110" height="145" rx="10" fill="#f4f0fb" stroke="#7656a8" stroke-width="2"/><text x="180" y="558" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="23" font-weight="700">任务三边界</text><text x="180" y="596" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">YOLOv8s INT8 Docker 环境与方案已准备；当前归档中没有真实 StarryOS 推理、检测结果到 AICtrl、RT-Thread 控制生效的同一次运行证据。</text><text x="180" y="628" font-family="Noto Sans CJK SC, Noto Sans, sans-serif" font-size="19">因此报告把任务三列为未完成，保留方案和缺陷说明。</text></svg>''',encoding='utf-8')
if __name__=='__main__': task1();rtthread();task2();completion();architecture()
