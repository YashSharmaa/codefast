#!/usr/bin/env python3
"""Write bench/k3/questions.verified.jsonl from bench/k3/questions.jsonl.

The verifier's decisions are data in this file: the gold edits (CORRECTIONS), the
notes-only amendments (NOTE_AMENDMENTS) and the per-question verification notes. Every
decision was reached by reading the source and by the independent searches described in
bench/k3/VERIFICATION.md; this script only applies them and checks that every source line
it cites still has the recorded text.

  python3 -I tools/verifier/make_verified.py
"""
import copy, json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
K3 = os.path.dirname(os.path.dirname(HERE))
LINUX = os.environ.get("LINUX", os.path.join(os.path.dirname(os.path.abspath(__file__)), *([".."] * (3 if "/verifier/" in __file__ else 2)), "repos", "linux"))
SRC = os.path.join(K3, "questions.jsonl")
DST = os.path.join(K3, "questions.verified.jsonl")
VERIFIED_ON = "2026-11-12"
COMMIT = "9d66f26fefacebfeed15de31c1123451a17ff119"


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def loc(path, line, expect):
    text = open(os.path.join(LINUX, path), errors="replace").read().split("\n")[line - 1]
    if norm(expect) not in norm(text):
        raise SystemExit("%s:%d does not contain %r: %r" % (path, line, expect, text))
    return "%s:%d: %s" % (path, line, norm(text))


# ---------------------------------------------------------------- family-level method notes
METHOD = {
    "F1": "Searched the whole tree for the name as a word (all file types) and read every hit: "
          "exactly one definition matches the question; prototypes, DECLARE_* lines and callers "
          "were told apart by reading.",
    "F2": "Read the definition at the gold line and checked every must_contain string against "
          "the source with whitespace collapsed; also read each prototype of the same name.",
    "F3": "Listed every occurrence of the name as a word in the whole tree (all file types, "
          "tools/ and Documentation/ included) and attributed each one to its enclosing function "
          "with a brace-depth scanner written for this check (tools/verifier/vk.py, not the "
          "author's column-0 heuristic). Every code occurrence followed by '(' is a gold call "
          "site, the definition or a declaration; the only occurrences without '(' are "
          "EXPORT_SYMBOL lines and comments, so there is no address-taken use. Also probed for "
          "token-pasted spellings (## before or after a fragment of the name): none.",
    "F4": "Read the whole body. Re-extracted the identifier( tokens with an independent "
          "comment/string stripper and function-extent scanner: the set equals callees + macros "
          "+ indirect. For every listed name searched for '#define NAME(' and for a function "
          "definition in generic and x86 code to check the function-versus-macro split.",
    "F5": "Read the table initializer and the function definition; searched every use of the "
          "function name and every '.member =' initializer in the subsystem to make sure the "
          "question has one answer.",
    "F6": "Searched every use of the member name as a word (not only '->member(') to find calls "
          "through the slot, including NULL checks, copies and static-call updates; attributed "
          "each call line to its enclosing function with the independent scanner; read the "
          "registration line.",
    "F7": "For every hop: the site line lies inside the 'from' function according to the "
          "independent scanner, and the line calls the 'to' function by name (call hops) or "
          "through the stated member / parameter (dispatch hops); the registration line names "
          "the 'to' function inside the stated table.",
    "F8": "Searched the tree for a shorter literal fragment of the message than the author used "
          "(all file types outside tools/ and Documentation/), read the source around the hit "
          "and resolved the enclosing function with the independent scanner; checked that the "
          "message in the question can really be produced by the format (prefixes from pr_fmt / "
          "dev_warn, %s arguments).",
    "F9": "Listed every occurrence of the member name as a word in scope and looked for writes "
          "with a wider pattern than the author's (plain and compound assignment, ++/--, "
          "address-of anywhere on the line, first argument of any wrapper call such as "
          "WRITE_ONCE / smp_store_release / this_cpu_write, and a member at end of line). For "
          "each gold writer line checked the enclosing function with the independent scanner "
          "and read the declaration that fixes the receiver's type; every non-gold write-like "
          "hit was read and belongs to another struct or is a read.",
}

# ---------------------------------------------------------------- per-question specifics
SPECIFIC = {
    "F1-01": "Only definition is SYSCALL_DEFINE6(io_uring_enter at io_uring/io_uring.c:2852; no COMPAT_SYSCALL_DEFINE variant exists.",
    "F1-02": "One DEFINE_ line (core.c:130). There are two DECLARE_ lines, not one: kernel/sched/sched.h:1448 and include/linux/sched.h:2427; the original note listed only the first. Note amended; gold unchanged.",
    "F1-03": "Gold file/line is right. The count of same-named definitions was wrong: the author's search '^static void init_once\\(' misses net/sunrpc/rpc_pipe.c:1219-1220, where 'static void' and 'init_once(void *foo)' are on separate lines. There are 32 definitions, not 31.",
    "F1-04": "Three hits of the static name: e1000e/netdev.c:5827 (answer), e1000/e1000_main.c:105 (forward declaration) and :3097 (legacy driver).",
    "F1-05": "No 'exc_page_fault(' signature exists; the other hits are two callers (coco/sev/vc-handle.c:117, entry/entry_fred.c:193), the Xen trap table and the DECLARE_ line.",
    "F1-06": "", "F1-07": "", "F1-08": "",
    "F2-01": "Prototype in include/linux/netdevice.h:3480 has identical parameter names.",
    "F2-02": "include/linux/mm.h has an extern prototype (3441) and a !CONFIG_MMU static inline stub (3452), both with the same parameter names.",
    "F2-03": "The definition is as stated. The generic prototype (include/linux/sched/task.h:83) has no parameter names, so an answer that quotes it fails the strings 'struct task_struct *p' and '... *args' although the signature is right; added also_accept_prototype.",
    "F2-04": "The definition is as stated. The prototype in kernel/sched/sched.h:3951 names the first parameter 'tsk', not 'p'; added also_accept_prototype so that a correct answer quoting the declaration is not scored wrong.",
    "F2-05": "The definition is as stated. The prototype in drivers/nvme/host/nvme.h:1006-1007 names the third parameter 'buf', not 'buffer'; added also_accept_prototype.",
    "F2-06": "", "F2-07": "", "F2-08": "",
    "F3-01": "17 occurrences: 4 call sites, definition, 1 declaration, 8 comments, 3 Documentation lines.",
    "F3-02": "12 call sites in 10 functions; do_swap_page and unuse_pte call twice.",
    "F3-03": "11 call sites in 7 functions; the remaining occurrences are the declaration, the definition and comments (drivers/nvme/host/tcp.c:1560, selftests).",
    "F3-04": "", "F3-05": "", "F3-06": "", "F3-07": "", "F3-08": "",
    "F4-01": "",
    "F4-02": "pte_alloc_one was listed under macros. On x86 it is a real function: arch/x86/include/asm/pgalloc.h:11 defines __HAVE_ARCH_PTE_ALLOC_ONE, which disables the generic macro (include/asm-generic/pgalloc.h:88-101), and arch/x86/mm/pgtable.c:16 defines pgtable_t pte_alloc_one(struct mm_struct *mm). Under the set's own rule (classified against x86 + generic headers) it is a callee; moved.",
    "F4-03": "", "F4-04": "", "F4-05": "", "F4-06": "",
    "F4-07": "All 25 callees have a function definition (several also have an empty static inline stub for the config-off case); the four macros are macros.",
    "F4-08": "",
    "F5-01": "Only one read_iter initializer in fs/ext4.",
    "F5-02": "enqueue_task_fair is also called directly once (fair.c:7090), which does not affect the table answer.",
    "F5-03": "", "F5-04": "",
    "F5-05": "Both vt_op() definitions read (main.c:878 with TDX, :881 without); vt_vcpu_run (main.c:154) forwards to vmx_vcpu_run for non-TD vCPUs.",
    "F5-06": "", "F5-07": "", "F5-08": "",
    "F6-01": "Member name task_tick has exactly three call lines in the tree, all in kernel/sched/core.c.",
    "F6-02": "One call through ops.dispatch_request; elevator.c:504 is a NULL check.",
    "F6-03": "One call through ->alloc_inode (fs/inode.c:338; line 337 is the NULL check).",
    "F6-04": "One call through ->issue (io_uring.c:1514).",
    "F6-05": "Exactly three call lines through ->ndo_start_xmit in the tree; the other uses are comparisons, a debug print and comments. The original note said the two extra sites 'cannot reach' loopback_xmit. That holds for ncm_tx_timeout (its own gadget netdev) but is too strong for rmnet_map_send_ack, which calls the op of skb->dev, the lower device rmnet is attached to; nothing in that function rules out lo. Note amended; scoring is unaffected because those sites are credited but not required.",
    "F6-06": "call_int_hook(file_permission, ...) appears once (security/security.c:2415).",
    "F6-07": "x86_pmu = intel_pmu at intel/core.c:8078 (core_pmu at :8076 for PMU version < 2 uses x86_pmu_handle_irq instead). One static_call(x86_pmu_handle_irq) site and one direct x86_pmu.handle_irq() site.",
    "F6-08": "Three calls through vm_ops->fault in the tree (mm/memory.c:5677, ipc/shm.c:545, fs/kernfs/file.c:394).",
    "F7-01": "", "F7-02": "", "F7-03": "", "F7-04": "", "F7-05": "", "F7-06": "",
    "F7-07": "dev.c:5088 is in the no-queue branch of __dev_queue_xmit.",
    "F7-08": "",
    "F8-01": "", "F8-02": "",
    "F8-03": "'TCP: ' comes from pr_fmt (tcp_input.c:65); 'Sending cookies' is the msg variable set at :7593.",
    "F8-04": "", "F8-05": "", "F8-06": "",
    "F8-07": "'I/O' is the blk_errors[] name for BLK_STS_IOERR (block/blk-core.c:184).",
    "F8-08": "",
    "F9-01": "All 127 occurrences of on_rq in .c/.h files outside tools/ and Documentation/ were read; the five listed lines are the only writes to task_struct.on_rq.",
    "F9-02": "All 52 occurrences (same scope) read; six writes to request_queue.nr_requests.",
    "F9-03": "All 144 occurrences (same scope) read; seven writes to mount.mnt_ns, the nsproxy.mnt_ns writes are in other functions.",
    "F9-04": "620 occurrences of sid under security/selinux; all 108 write-like lines read. The receivers of all 17 gold lines are declared struct inode_security_struct *; ipc_init_security's isec (hooks.c:6465) is a struct ipc_security_struct *.",
    "F9-05": "All 53 occurrences (same scope) read; ten write lines in nine functions.",
    "F9-06": "264 occurrences tree-wide in 53 files; all 65 under drivers/nvme (10 files) read, and the other 43 files are unrelated drivers with their own structs; the 15 write lines are in 14 functions. No file outside drivers/nvme uses struct nvme_ctrl's queue_count.",
    "F9-07": "929 occurrences of sp in .c/.h files under arch/x86 (plus assembly); the 64 write-like lines were listed and every one whose receiver is not pt_regs was read. Three C writers of thread.sp; TASK_threadsp is written in __switch_to_asm (entry_64.S:191, entry_32.S:689) and only read in head_64.S.",
    "F9-08": "All 51 occurrences (same scope) read; two writes to io_ring_ctx.cq_entries.",
}

# ---------------------------------------------------------------- gold corrections
def fix_f1_03(q):
    q["gold"]["definitions_in_tree"] = 32
    q["notes"] = ("Disambiguation by subsystem: 32 files define a static void init_once() "
                  "(block/bdev.c, net/socket.c, fs/inode.c, 20+ filesystems, ipc/mqueue.c, "
                  "drivers/dax/super.c, net/sunrpc/rpc_pipe.c). 31 of them have the header on one "
                  "line; net/sunrpc/rpc_pipe.c:1219-1220 splits it over two lines and is missed by "
                  "a search for '^static void init_once\\('. Only the ext4 one is correct.")
    q["evidence"].append({
        "cmd": "sed -n '1219,1220p' net/sunrpc/rpc_pipe.c",
        "lines": [loc("net/sunrpc/rpc_pipe.c", 1219, "static void"),
                  loc("net/sunrpc/rpc_pipe.c", 1220, "init_once(void *foo)")],
        "added_by": "verifier"})
    return ["gold.definitions_in_tree 31 -> 32 (net/sunrpc/rpc_pipe.c:1219-1220 has the header on two lines)",
            "notes rewritten accordingly", "one evidence item added"]


def add_proto(q, path, line, must, lines):
    q["gold"]["also_accept_prototype"] = {"file": path, "line": line, "must_contain": must}
    q["evidence"].append({"cmd": "sed -n '%d,%dp' %s" % (lines[0][0], lines[-1][0], path),
                          "lines": [loc(path, n, t) for n, t in lines], "added_by": "verifier"})
    q["notes"] = (q["notes"] + " " if q["notes"] else "") + (
        "A prototype of this function uses different (or no) parameter names; an answer that "
        "satisfies gold.also_accept_prototype.must_contain instead of gold.must_contain is also correct.")
    return ["gold.also_accept_prototype added (%s:%d); gold.must_contain unchanged" % (path, line),
            "note sentence added", "one evidence item added"]


def fix_f2_03(q):
    return add_proto(q, "include/linux/sched/task.h", 83,
                     ["int", "struct task_struct *", "const struct kernel_clone_args *"],
                     [(83, "extern int copy_thread(struct task_struct *, const struct kernel_clone_args *);")])


def fix_f2_04(q):
    return add_proto(q, "kernel/sched/sched.h", 3951,
                     ["int", "struct task_struct *tsk", "unsigned int state", "int wake_flags"],
                     [(3951, "extern int try_to_wake_up(struct task_struct *tsk, unsigned int state, int wake_flags);")])


def fix_f2_05(q):
    return add_proto(q, "drivers/nvme/host/nvme.h", 1006,
                     ["int", "struct request_queue *q", "struct nvme_command *cmd", "void *buf", "unsigned bufflen"],
                     [(1006, "int nvme_submit_sync_cmd(struct request_queue *q, struct nvme_command *cmd,"),
                      (1007, "void *buf, unsigned bufflen);")])


def fix_f4_02(q):
    g = q["gold"]
    assert "pte_alloc_one" in g["macros"] and g["callees"][0] == "pmd_none"
    g["macros"].remove("pte_alloc_one")
    g["callees"].insert(1, "pte_alloc_one")  # order of first use: after pmd_none
    q["notes"] = q["notes"].replace(
        "pte_alloc_one is a macro wrapping pte_alloc_one_noprof (include/asm-generic/pgalloc.h:101);",
        "pte_alloc_one is a real function on x86 (arch/x86/mm/pgtable.c:16; x86 defines "
        "__HAVE_ARCH_PTE_ALLOC_ONE, which disables the generic macro in "
        "include/asm-generic/pgalloc.h:101 that wraps pte_alloc_one_noprof on other architectures; "
        "pte_alloc_one_noprof is accepted as an alternative name for it);")
    assert "real function on x86" in q["notes"]
    g["callee_aliases"] = {"pte_alloc_one": ["pte_alloc_one_noprof"]}
    q["evidence"].append({
        "cmd": "rg -n 'pte_alloc_one\\(|__HAVE_ARCH_PTE_ALLOC_ONE' arch/x86/mm/pgtable.c "
               "arch/x86/include/asm/pgalloc.h include/asm-generic/pgalloc.h",
        "lines": [loc("arch/x86/mm/pgtable.c", 16, "pgtable_t pte_alloc_one(struct mm_struct *mm)"),
                  loc("arch/x86/include/asm/pgalloc.h", 11, "#define __HAVE_ARCH_PTE_ALLOC_ONE"),
                  loc("include/asm-generic/pgalloc.h", 88, "#ifndef __HAVE_ARCH_PTE_ALLOC_ONE"),
                  loc("include/asm-generic/pgalloc.h", 101, "#define pte_alloc_one(...)")],
        "added_by": "verifier"})
    return ["gold: pte_alloc_one moved from macros (optional) to callees (required); 9 -> 10 function callees",
            "gold.callee_aliases added: pte_alloc_one_noprof is accepted for pte_alloc_one (what the generic macro expands to on non-x86 architectures)",
            "notes corrected", "one evidence item added"]


CORRECTIONS = {"F1-03": fix_f1_03, "F2-03": fix_f2_03, "F2-04": fix_f2_04, "F2-05": fix_f2_05,
               "F4-02": fix_f4_02}


# ---------------------------------------------------------------- notes-only amendments
def amend_f1_02(q):
    old = "kernel/sched/sched.h:1448 is the DECLARE_ (extern) and is not the definition."
    assert old in q["notes"]
    q["notes"] = q["notes"].replace(old, "kernel/sched/sched.h:1448 and include/linux/sched.h:2427 are "
                                         "DECLARE_ (extern) lines and are not the definition.")
    q["evidence"].append({"cmd": "rg -n 'PER_CPU\\w*\\(struct rq, runqueues\\)' include/linux/sched.h",
                          "lines": [loc("include/linux/sched.h", 2427, "DECLARE_PER_CPU_SHARED_ALIGNED(struct rq, runqueues);")],
                          "added_by": "verifier"})
    return ["notes: second DECLARE_ line (include/linux/sched.h:2427) added to the list of wrong candidates; gold unchanged",
            "one evidence item added"]


def amend_f6_05(q):
    old = "which invoke ndo_start_xmit on their own devices and cannot reach loopback_xmit;"
    assert old in q["notes"]
    q["notes"] = q["notes"].replace(old, "which are not the generic transmit path: ncm_tx_timeout calls the op of "
                                         "the gadget's own netdev and cannot reach loopback_xmit, "
                                         "rmnet_map_send_ack calls the op of skb->dev (the lower device rmnet is "
                                         "attached to, normally a modem data device);")
    return ["notes: 'cannot reach loopback_xmit' narrowed to the NCM site; gold unchanged"]


NOTE_AMENDMENTS = {"F1-02": amend_f1_02, "F6-05": amend_f6_05}


def main():
    out = []
    for line in open(SRC):
        q = json.loads(line)
        orig_gold = copy.deepcopy(q["gold"])
        changes = []
        if q["id"] in CORRECTIONS:
            changes = CORRECTIONS[q["id"]](q)
            assert q["gold"] != orig_gold
            status = "corrected"
        else:
            if q["id"] in NOTE_AMENDMENTS:
                changes = NOTE_AMENDMENTS[q["id"]](q)
            assert q["gold"] == orig_gold
            status = "confirmed"
        notes = METHOD[q["family"]]
        if SPECIFIC.get(q["id"]):
            notes += " " + SPECIFIC[q["id"]]
        q["verification"] = {"status": status, "changes": changes, "verifier_notes": notes,
                             "verified_on": VERIFIED_ON, "commit": COMMIT}
        out.append(q)
    assert len(out) == 72 and set(SPECIFIC) == {q["id"] for q in out}
    with open(DST, "w") as f:
        for q in out:
            f.write(json.dumps(q, ensure_ascii=False) + "\n")
    n = {}
    for q in out:
        n[q["verification"]["status"]] = n.get(q["verification"]["status"], 0) + 1
    print("wrote %s: %d questions %s" % (DST, len(out), n))


if __name__ == "__main__":
    main()
