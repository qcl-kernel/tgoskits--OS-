#!/usr/bin/env python3
"""Generate one honest, combined overview of the three work packages."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "figures"
FONT = "Noto Sans CJK SC, Noto Sans, sans-serif"


def text(x, y, value, size=20, weight="400", color="#18324b"):
    escaped = value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    return f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{size}" font-weight="{weight}" fill="{color}">{escaped}</text>'


def build():
    svg = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1500" height="980" viewBox="0 0 1500 980">',
        '<rect width="1500" height="980" fill="#f7f9fc"/>',
        text(60, 62, "作品整体链路：RT-Thread / Zephyr / AICtrl-IP / YOLOv8s", 32, "700"),
        text(60, 96, "三项任务的关系与证据边界；任务三保留为未完成，不把方案当作实测结果。", 17, "400", "#52616b"),
        '<rect x="60" y="130" width="1380" height="220" rx="10" fill="#eef8f1" stroke="#4c956c" stroke-width="3"/>',
        text(90, 172, "任务一：实时性与运行时基线", 24, "700", "#2f6f4e"),
        text(90, 208, "RT-Thread 为主要对照对象；Zephyr 作为辅助功能性基线。", 18),
        '<rect x="90" y="230" width="390" height="85" rx="6" fill="#fff" stroke="#4c956c"/>',
        text(112, 262, "RT-Thread native / AxVisor", 19, "700"),
        text(112, 291, "相同场景 P99 周期抖动对照", 16, "400", "#52616b"),
        '<rect x="520" y="230" width="390" height="85" rx="6" fill="#fff" stroke="#8aa9bd" stroke-dasharray="7 5"/>',
        text(542, 262, "Zephyr native_sim", 19, "700"),
        text(542, 291, "辅助功能性 baseline，不代表真实 guest 延迟", 16, "400", "#52616b"),
        '<rect x="950" y="230" width="450" height="85" rx="6" fill="#fff" stroke="#4c956c"/>',
        text(972, 262, "StarryOS 周期 probe", 19, "700"),
        text(972, 291, "5 ms / 30 min / 4 runs，CSV 已归档", 16, "400", "#52616b"),
        '<rect x="60" y="380" width="1380" height="220" rx="10" fill="#eaf3fb" stroke="#2468a8" stroke-width="3"/>',
        text(90, 422, "任务二：通信与可靠性", 24, "700", "#2468a8"),
        text(90, 458, "StarryOS 与 RT-Thread 通过 direct TAP/bridge 运行 AICtrl/IP v2。", 18),
        '<rect x="100" y="490" width="300" height="70" rx="6" fill="#fff" stroke="#2468a8"/>',
        text(125, 532, "StarryOS client", 20, "700"),
        '<line x1="400" y1="525" x2="525" y2="525" stroke="#2468a8" stroke-width="4"/><polygon points="525,525 510,516 510,534" fill="#2468a8"/>',
        '<rect x="525" y="490" width="430" height="70" rx="6" fill="#fff" stroke="#2468a8"/>',
        text(550, 520, "AICtrl/IP v2 · direct TAP/bridge", 19, "700"),
        text(550, 545, "ACK / retry / duplicate / restart recovery", 15, "400", "#52616b"),
        '<line x1="955" y1="525" x2="1080" y2="525" stroke="#2468a8" stroke-width="4"/><polygon points="1080,525 1065,516 1065,534" fill="#2468a8"/>',
        '<rect x="1080" y="490" width="300" height="70" rx="6" fill="#fff" stroke="#2468a8"/>',
        text(1105, 532, "RT-Thread server", 20, "700"),
        text(100, 586, "已验证：4,000,000 请求全部成功，约 35.17 分钟，failure / timeout / retry / duplicate = 0。", 16, "400", "#2468a8"),
        '<rect x="60" y="630" width="1380" height="220" rx="10" fill="#fff4e8" stroke="#e07a2d" stroke-width="3"/>',
        text(90, 672, "任务三：大模型/YOLO 到控制联动", 24, "700", "#b45c20"),
        text(90, 708, "目标链路：图像输入 → YOLOv8s INT8 → 检测结果映射 → AICtrl/IP → RT-Thread 控制。", 18),
        '<rect x="100" y="740" width="260" height="70" rx="6" fill="#fff" stroke="#e07a2d" stroke-dasharray="7 5"/>',
        text(125, 782, "图像 / 摄像头", 20, "700"),
        '<line x1="360" y1="775" x2="470" y2="775" stroke="#e07a2d" stroke-width="4"/><polygon points="470,775 455,766 455,784" fill="#e07a2d"/>',
        '<rect x="470" y="740" width="300" height="70" rx="6" fill="#fff" stroke="#e07a2d" stroke-dasharray="7 5"/>',
        text(495, 782, "YOLOv8s INT8 推理", 20, "700"),
        '<line x1="770" y1="775" x2="880" y2="775" stroke="#e07a2d" stroke-width="4"/><polygon points="880,775 865,766 865,784" fill="#e07a2d"/>',
        '<rect x="880" y="740" width="300" height="70" rx="6" fill="#fff" stroke="#e07a2d" stroke-dasharray="7 5"/>',
        text(905, 782, "策略映射 / AICtrl", 20, "700"),
        '<line x1="1180" y1="775" x2="1290" y2="775" stroke="#e07a2d" stroke-width="4"/><polygon points="1290,775 1275,766 1275,784" fill="#e07a2d"/>',
        '<rect x="1290" y="740" width="110" height="70" rx="6" fill="#fff" stroke="#e07a2d" stroke-dasharray="7 5"/>',
        text(1308, 782, "控制", 20, "700"),
        text(90, 836, "当前状态：环境和接口方案已准备，但没有真实同次运行的推理、发包、控制生效证据。", 16, "400", "#b45c20"),
        '<rect x="60" y="880" width="1380" height="60" rx="8" fill="#18324b"/>',
        text(90, 918, "证据状态：任务一 PARTIAL    ·    任务二 PASS    ·    任务三 NOT DONE", 20, "700", "#ffffff"),
        '</svg>',
    ]
    (OUT / "overall_system_overview.svg").write_text("".join(svg), encoding="utf-8")
    print("overall_system_overview.svg generated")


if __name__ == "__main__":
    build()
