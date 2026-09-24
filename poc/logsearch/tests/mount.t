# DATA_MODEL.md section 6 — 宣言されたマウントと、置き場所の選び方。
#
# ここで固めたいのは 2 つ。**理解できなかった行を半端に適用しない**こと
# (quota を取り違えたマウントは、無いマウントより高くつく) と、
# **配置がラウンドロビンではなく使用率で決まる**こと — 容量の違う
# ディスクを混ぜるのが普通なので、交互に使うと小さい方が先に埋まる。

import std.fs
import std.io
import std.path
import mount

fn cfg(text: str) -> String {
    val out = String::from_str(text)
    out
}

fn sized(spec: str) -> u64 {
    val s = String::from_str(spec)
    val got = mount::parse_size(&s)
    match got {
        Option::Some(v) => { v }
        Option::None => { panic("parse_size({spec}) should have parsed") }
    }
}

fn unsized_ok(spec: str) -> bool {
    val s = String::from_str(spec)
    val got = mount::parse_size(&s)
    match got {
        Option::Some(v) => { false }
        Option::None => { true }
    }
}

# ---------------------------------------------------------------------

test "a size is read in powers of 1024" {
    assert_eq(sized("512"), 512u64)
    assert_eq(sized("1K"), 1024u64)
    assert_eq(sized("8M"), 8388608u64)
    assert_eq(sized("100G"), 107374182400u64)
    assert_eq(sized("2T"), 2199023255552u64)
    # Lower case is the same number.
    assert_eq(sized("100g"), 107374182400u64)

    assert(unsized_ok(""), "an empty size is not a size")
    assert(unsized_ok("G"), "a bare suffix is not a size")
    assert(unsized_ok("100X"), "an unknown suffix is not a size")
    assert(unsized_ok("-1"), "a negative size is not a size")
}

test "a configuration names the mounts and nothing else does" {
    var ms = MountSet::new()
    val text = cfg("# the disks\n\nmount /var/log/logsearch/a  quota=100G\nmount /mnt/disk2/logsearch  quota=400G\nmount /mnt/disk3/logsearch  quota=400G  readonly\n")
    val bad = mount::parse_config(&text, &mut ms)
    assert_eq(bad, 0u64)
    assert_eq(ms.size(), 3u64)

    val first = ms.path_of(0u64)
    val want = String::from_str("/var/log/logsearch/a")
    assert(first.eq(&want), "the first mount should be the first line")
    assert_eq(ms.quota_of(0u64), 107374182400u64)
    assert(!ms.is_readonly(0u64), "mount a is writable")
    assert(ms.is_readonly(2u64), "disk3 was declared readonly")
    assert_eq(mount::state_name(ms.state_of(0u64)), "active")
}

# 半端に適用しない。quota が読めない行は**マウントを作らない**。
test "a line that is not understood adds no mount" {
    var ms = MountSet::new()
    val text = cfg("mount /good  quota=1G\nmount /bad  quota=100X\nmount /nolimit\nsomething else entirely\nmount /alsogood  quota=2G\n")
    val bad = mount::parse_config(&text, &mut ms)
    assert_eq(bad, 3u64)
    assert_eq(ms.size(), 2u64)
    val a = ms.path_of(0u64)
    val wa = String::from_str("/good")
    assert(a.eq(&wa), "the good line before the bad one survives")
    val b = ms.path_of(1u64)
    val wb = String::from_str("/alsogood")
    assert(b.eq(&wb), "parsing continues past a bad line")
}

# 容量の違うディスクを混ぜる形。ラウンドロビンなら 100G の方が先に
# 埋まるが、使用率で選べば大きい方に寄る。
test "placement follows the share used, not the turn" {
    var ms = MountSet::new()
    val text = cfg("mount /small  quota=100G\nmount /big  quota=400G\n")
    val bad = mount::parse_config(&text, &mut ms)
    assert_eq(bad, 0u64)

    # 両方 0 なら宣言順。
    val empty = ms.pick()
    match empty {
        Option::Some(i) => { assert_eq(i, 0u64) }
        Option::None => { panic("an empty set of mounts should still be writable") }
    }

    # 小さい方に 10G (10%)、大きい方に 20G (5%)。**バイト数では
    # 大きい方が多い**のに、選ばれるのは大きい方。
    ms.set_used(0u64, 10737418240u64)
    ms.set_used(1u64, 21474836480u64)
    assert_eq(ms.permille(0u64), 100u64)
    assert_eq(ms.permille(1u64), 50u64)
    val next = ms.pick()
    match next {
        Option::Some(i) => { assert_eq(i, 1u64) }
        Option::None => { panic("both mounts are writable") }
    }
}

test "a mount that is full or readonly is not picked" {
    var ms = MountSet::new()
    val text = cfg("mount /ro  quota=100G  readonly\nmount /rw  quota=100G\n")
    val bad = mount::parse_config(&text, &mut ms)
    assert_eq(bad, 0u64)

    val only = ms.pick()
    match only {
        Option::Some(i) => { assert_eq(i, 1u64) }
        Option::None => { panic("the writable mount should be available") }
    }

    # quota に達したら full になり、選ばれなくなる。
    ms.set_used(1u64, 107374182400u64)
    assert_eq(mount::state_name(ms.state_of(1u64)), "full")
    val none = ms.pick()
    match none {
        Option::Some(i) => { panic("a full mount must not be picked") }
        Option::None => { }
    }
}

test "a degraded mount stops taking writes" {
    var ms = MountSet::new()
    val text = cfg("mount /a  quota=1G\nmount /b  quota=1G\n")
    val bad = mount::parse_config(&text, &mut ms)
    assert_eq(bad, 0u64)
    ms.mark(0u64, MountState::Degraded)
    val pick = ms.pick()
    match pick {
        Option::Some(i) => { assert_eq(i, 1u64) }
        Option::None => { panic("the healthy mount should be available") }
    }
}

# ---------------------------------------------------------------------

test "a mount directory keeps the identity it was given" {
    val dir = "build/mount-identity"
    val meta = "{dir}/meta"
    val made = fs::mkdir_all(meta)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("mkdir {meta}: {e}") }
    }
    val f = mount::meta_file(dir)
    val gone = fs::remove_file(f.to_str())
    match gone {
        Result::Ok(u) => { }
        Result::Err(e) => { }
    }

    val first = mount::ensure_meta(dir, "test")
    assert(first.ok, "ensure_meta should write a mount.json")
    assert_eq(first.format, mount::META_FORMAT)
    # 束縛してから使う。struct フィールドへの `&` を引数位置に直接
    # 書く形は compiled レーンが lower できない。
    val born = first.uuid.clone()
    assert_eq(born.len(), 32u64)

    # 2 回目は**書き直さない**。ここで新しい uuid を振ると、同じ
    # ディレクトリが起動のたびに別のマウントに見える。
    val again = mount::ensure_meta(dir, "test")
    assert(again.ok, "the second open should read what the first wrote")
    val reopened = again.uuid.clone()
    assert(reopened.eq(&born), "the identity must not change on reopen")

    assert(mount::identity_matches(&again, &born),
           "the same directory should match what was remembered")
    val other = String::from_str("00000000000000000000000000000000")
    assert(!mount::identity_matches(&again, &other),
           "a different identity must not match")
}

# 「分からない」の答えは**一致しない**である。取り違えて上書きするのが
# この設計で最も高くつく事故なので、読めない mount.json は通さない。
test "an unreadable identity does not pass for a match" {
    val absent = mount::read_meta("build/mount-does-not-exist")
    assert(!absent.ok, "a missing mount.json is not an identity")
    val anything = String::from_str("00000000000000000000000000000000")
    assert(!mount::identity_matches(&absent, &anything),
           "an unknown identity must not match")

    # 空の記憶とも一致しない (両方とも空なら通る、では困る)。
    #
    # **自分のディレクトリを使う。** 上のテストと共有すると、`-j4` で
    # 並行に走ったときに片方の `remove_file` がもう片方の書き込みと
    # 競り、`ensure_meta` が 5 回に 4 回落ちる。テストが触るファイルは
    # テストごとに分けること。
    val dir = "build/mount-identity-known"
    val known = mount::ensure_meta(dir, "test")
    val nothing = String::new()
    assert(!mount::identity_matches(&known, &nothing),
           "an empty memory matches nothing")
}
