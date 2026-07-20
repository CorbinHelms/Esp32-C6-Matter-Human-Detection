#!/usr/bin/env python3
"""Simple GUI wrapper around scripts/provision.py.

Run with a new board plugged in:  scripts/provision_gui.py

Pick the serial port, hit "Provision new unit", watch the log; when it
finishes you get the unit's pairing code + QR and one-click access to its
printable pamphlet. Existing units are listed below with re-render / open
buttons. All the actual work is done by provision.py as a subprocess, so
the CLI and GUI can't drift apart.
"""

import glob
import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import tkinter as tk
from tkinter import ttk, messagebox

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROVISION = os.path.join(REPO, "scripts", "provision.py")
REGISTRY = os.path.join(REPO, "hardware", "units", "registry.json")
UNITS_DIR = os.path.join(REPO, "hardware", "units")


def list_ports():
    return sorted(glob.glob("/dev/ttyACM*") + glob.glob("/dev/ttyUSB*"))


def load_registry():
    try:
        with open(REGISTRY) as f:
            return json.load(f)
    except (OSError, ValueError):
        return []


def open_path(path):
    subprocess.Popen(["xdg-open", path],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


class App:
    def __init__(self, root):
        self.root = root
        root.title("Sensor provisioning")
        root.minsize(560, 640)
        self.proc = None
        self.lines = queue.Queue()

        pad = {"padx": 10, "pady": 4}
        top = ttk.Frame(root)
        top.pack(fill="x", **pad)

        ttk.Label(top, text="Serial port:").pack(side="left")
        self.port = tk.StringVar()
        self.port_box = ttk.Combobox(top, textvariable=self.port, width=18)
        self.port_box.pack(side="left", padx=6)
        ttk.Button(top, text="Rescan", command=self.rescan).pack(side="left")
        self.dry = tk.BooleanVar(value=False)
        ttk.Checkbutton(top, text="Dry run (no flash)",
                        variable=self.dry).pack(side="left", padx=10)

        self.go = ttk.Button(root, text="Provision new unit",
                             command=self.provision)
        self.go.pack(fill="x", padx=10, pady=(2, 4), ipady=6)

        self.status = ttk.Label(root, text="Plug in a new board and hit "
                                "Provision.", anchor="w")
        self.status.pack(fill="x", **pad)

        # result card (hidden until a run completes)
        self.card = ttk.LabelFrame(root, text="Last provisioned")
        self.card_qr = ttk.Label(self.card)
        self.card_qr.pack(side="left", padx=8, pady=8)
        card_txt = ttk.Frame(self.card)
        card_txt.pack(side="left", fill="both", expand=True, padx=4, pady=8)
        self.card_title = ttk.Label(card_txt, font=("TkDefaultFont", 12, "bold"))
        self.card_title.pack(anchor="w")
        self.card_code = ttk.Label(card_txt, font=("TkFixedFont", 16))
        self.card_code.pack(anchor="w", pady=4)
        row = ttk.Frame(card_txt)
        row.pack(anchor="w", pady=2)
        ttk.Button(row, text="Open pamphlet PDF",
                   command=lambda: self.open_unit_file("pamphlet.pdf")).pack(
            side="left")
        ttk.Button(row, text="Open folder",
                   command=lambda: self.open_unit_file("")).pack(
            side="left", padx=6)
        self.card_unit = None
        self._qr_img = None

        # log
        logf = ttk.LabelFrame(root, text="Log")
        logf.pack(fill="both", expand=True, **pad)
        self.log = tk.Text(logf, height=12, state="disabled", wrap="none",
                           font=("TkFixedFont", 9))
        yscroll = ttk.Scrollbar(logf, command=self.log.yview)
        self.log.configure(yscrollcommand=yscroll.set)
        yscroll.pack(side="right", fill="y")
        self.log.pack(fill="both", expand=True)

        # unit table
        unitf = ttk.LabelFrame(root, text="Provisioned units")
        unitf.pack(fill="x", **pad)
        cols = ("unit", "date", "code", "discriminator")
        self.table = ttk.Treeview(unitf, columns=cols, show="headings",
                                  height=5, selectmode="browse")
        for c, w in zip(cols, (60, 100, 130, 110)):
            self.table.heading(c, text=c.capitalize())
            self.table.column(c, width=w, anchor="w")
        self.table.pack(side="left", fill="x", expand=True)
        btns = ttk.Frame(unitf)
        btns.pack(side="left", fill="y", padx=6)
        ttk.Button(btns, text="Open PDF",
                   command=self.open_selected_pdf).pack(fill="x", pady=2)
        ttk.Button(btns, text="Re-render",
                   command=self.regen_selected).pack(fill="x", pady=2)

        self.rescan()
        self.refresh_units()
        self.root.after(100, self.drain_log)

    # ---------------------------------------------------------------- helpers

    def rescan(self):
        ports = list_ports()
        self.port_box["values"] = ports
        if ports and self.port.get() not in ports:
            self.port.set(ports[0])
        if not ports:
            self.port.set("")

    def append_log(self, text):
        self.log.configure(state="normal")
        self.log.insert("end", text)
        self.log.see("end")
        self.log.configure(state="disabled")

    def drain_log(self):
        try:
            while True:
                item = self.lines.get_nowait()
                if item is None:            # subprocess finished
                    self.finished()
                else:
                    self.append_log(item)
        except queue.Empty:
            pass
        self.root.after(100, self.drain_log)

    def refresh_units(self):
        self.table.delete(*self.table.get_children())
        for u in reversed(load_registry()):
            self.table.insert("", "end", iid=str(u["unit"]), values=(
                "%03d" % u["unit"], u["date"],
                "%s-%s-%s" % (u["manual_code"][0:4], u["manual_code"][4:7],
                              u["manual_code"][7:11]),
                u["discriminator"]))

    def selected_unit(self):
        sel = self.table.selection()
        if not sel:
            messagebox.showinfo("No unit selected",
                                "Pick a unit in the table first.")
            return None
        return int(sel[0])

    def unit_dir(self, unit_no):
        return os.path.join(UNITS_DIR, "unit-%03d" % unit_no)

    def open_selected_pdf(self):
        u = self.selected_unit()
        if u is not None:
            open_path(os.path.join(self.unit_dir(u), "pamphlet.pdf"))

    def open_unit_file(self, name):
        if self.card_unit is not None:
            open_path(os.path.join(self.unit_dir(self.card_unit), name))

    # ------------------------------------------------------------------- runs

    def start_proc(self, args, status):
        if self.proc:
            return
        self.status.configure(text=status)
        self.go.state(["disabled"])
        self.append_log("$ " + " ".join(args) + "\n")

        def pump():
            self.proc = subprocess.Popen(
                args, cwd=REPO, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, text=True, bufsize=1)
            for line in self.proc.stdout:
                self.lines.put(line)
            self.proc.wait()
            self.lines.put(None)

        threading.Thread(target=pump, daemon=True).start()

    def provision(self):
        args = [sys.executable, PROVISION]
        if self.dry.get():
            args.append("--no-flash")
        elif not self.port.get():
            messagebox.showerror(
                "No serial port",
                "No board found on /dev/ttyACM* or /dev/ttyUSB*.\n"
                "Plug the board in and hit Rescan, or tick Dry run.")
            return
        else:
            args += ["--port", self.port.get()]
        self.expect_new_unit = True
        self.start_proc(args, "Provisioning... (build + flash take a bit)")

    def regen_selected(self):
        u = self.selected_unit()
        if u is not None:
            self.expect_new_unit = False
            self.start_proc([sys.executable, PROVISION, "--regen", str(u)],
                            "Re-rendering pamphlet for unit %03d..." % u)

    def finished(self):
        code = self.proc.returncode if self.proc else -1
        self.proc = None
        self.go.state(["!disabled"])
        self.refresh_units()
        if code != 0:
            self.status.configure(text="Failed (exit %d) - see the log." % code)
            return
        self.status.configure(text="Done.")
        if not getattr(self, "expect_new_unit", False):
            return
        reg = load_registry()
        if not reg:
            return
        unit = reg[-1]
        self.card_unit = unit["unit"]
        self.card_title.configure(text="Unit %03d - %s" %
                                  (unit["unit"], unit["date"]))
        self.card_code.configure(text="%s-%s-%s" % (
            unit["manual_code"][0:4], unit["manual_code"][4:7],
            unit["manual_code"][7:11]))
        self.show_qr(unit["qr"])
        self.card.pack(fill="x", padx=10, pady=4,
                       after=self.status)
        self.status.configure(
            text="Done. Print the pamphlet; power-cycle the board and check "
                 "the boot log shows the same code.")

    def show_qr(self, payload):
        try:
            png = os.path.join(tempfile.gettempdir(), "provision-qr.png")
            subprocess.run(["qrencode", "-t", "png", "-s", "4", "-m", "2",
                            "-o", png, payload], check=True)
            self._qr_img = tk.PhotoImage(file=png)
            self.card_qr.configure(image=self._qr_img)
        except Exception:
            self.card_qr.configure(image="", text="(QR render failed)")


def main():
    root = tk.Tk()
    try:
        ttk.Style().theme_use("clam")
    except tk.TclError:
        pass
    App(root)
    root.mainloop()


if __name__ == "__main__":
    main()
