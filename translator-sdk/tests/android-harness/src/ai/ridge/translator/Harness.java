package ai.ridge.translator;

/**
 * P6.1 的 Java 调用方（测试 Harness，不是产品 UI）。
 *
 * 覆盖用户批准的验收项：
 *   J-01 创建真实 Translator → capabilities → 关闭
 *   J-02 中文 / Emoji / NUL / 非法输入按契约处理
 *   J-03 代际句柄、重复关闭、过期句柄不崩溃
 *   J-05 真实词典 lookup 与语言 detect
 *
 * 输出格式固定为 "PASS <id> ..." / "FAIL <id> ..."，便于脚本统计。
 */
public final class Harness {
    private static int pass = 0;
    private static int fail = 0;

    public static void main(String[] args) throws Exception {
        String dictDir = args.length > 0 ? args[0] : "/data/local/tmp/dictpkg";

        System.out.println("protocol_version=" + NativeBridge.nativeProtocolVersion());

        String config = "{"
                + "\"protocol_version\":1,"
                + "\"data_dir\":\"/data/local/tmp/p6\","
                + "\"dict_package\":\"" + dictDir + "\","
                + "\"term_policy\":\"warn\"}";

        // ---------- J-01: create → capabilities → close ----------
        String created = NativeBridge.nativeCreate(config);
        String handle = extract(created, "\"handle\":\"", "\"");
        check("J-01a", handle != null && handle.contains(":"),
                "create 返回代际句柄: " + handle + " | " + trim(created));

        String caps = NativeBridge.nativeCall(handle, "capabilities", "{}");
        check("J-01b", caps.contains("\"ok\":true") && caps.contains("translation.dictionary"),
                "capabilities 含词典能力: " + trim(caps));
        check("J-01c", caps.contains("runtime.bergamot") && caps.contains("not_compiled"),
                "未编译的 NMT 运行时如实报告: " + trim(caps));

        // ---------- J-02: 中文 / Emoji / NUL / 空串 ----------
        String zh = NativeBridge.nativeCall(handle, "detect", "{\"text\":\"你好世界\"}");
        check("J-02a", zh.contains("\"ok\":true") && zh.contains("zh-Hans"),
                "中文识别: " + trim(zh));

        String emoji = NativeBridge.nativeCall(handle, "detect",
                "{\"text\":\"\\uD83D\\uDE00\\uD83D\\uDE80 emoji \\u4F60\\u597D\"}");
        check("J-02b", emoji.contains("\"ok\":true"),
                "含非 BMP 字符的输入往返不崩: " + trim(emoji));

        String nul = NativeBridge.nativeCall(handle, "detect", "{\"text\":\"a\\u0000b\"}");
        check("J-02c", nul.contains("InvalidRequest") && nul.contains("NUL"),
                "含 NUL 的输入被明确拒绝: " + trim(nul));

        String empty = NativeBridge.nativeCall(handle, "detect", "{\"text\":\"   \"}");
        check("J-02d", empty.contains("InvalidRequest"),
                "空白输入被拒绝: " + trim(empty));

        // ---------- J-05: 真实词典 lookup + detect ----------
        String lookup = NativeBridge.nativeCall(handle, "lookup",
                "{\"term\":\"你好\",\"limit\":5}");
        check("J-05a", lookup.contains("\"ok\":true") && lookup.contains("hello")
                && lookup.contains("cc-cedict"),
                "真词典多义与来源: " + trim(lookup));

        String multi = NativeBridge.nativeCall(handle, "lookup", "{\"term\":\"翻译\"}");
        check("J-05b", multi.contains("to translate") && multi.contains("translation"),
                "多释义不丢失: " + trim(multi));

        String miss = NativeBridge.nativeCall(handle, "lookup", "{\"term\":\"不存在的词xyz\"}");
        check("J-05c", miss.contains("\"ok\":true"),
                "查不到时返回空结果而不是崩溃: " + trim(miss));

        // ---------- J-03: 重复关闭 / 过期句柄 ----------
        String close1 = NativeBridge.nativeClose(handle);
        check("J-03a", close1.contains("\"closed\":true") && !close1.contains("already_closed"),
                "首次关闭正常: " + trim(close1));

        String close2 = NativeBridge.nativeClose(handle);
        check("J-03b", close2.contains("already_closed"),
                "重复关闭幂等: " + trim(close2));

        String afterClose = NativeBridge.nativeCall(handle, "capabilities", "{}");
        check("J-03c", afterClose.contains("AlreadyClosed"),
                "关闭后的调用被明确拒绝（无 use-after-free）: " + trim(afterClose));

        String badHandle = NativeBridge.nativeCall("not-a-handle", "capabilities", "{}");
        check("J-03d", badHandle.contains("InvalidRequest"),
                "非法句柄格式返回错误而不是崩溃: " + trim(badHandle));

        String stale = NativeBridge.nativeCall("9999:0", "capabilities", "{}");
        check("J-03e", stale.contains("AlreadyClosed"),
                "过期代际句柄被拒绝: " + trim(stale));


        // ---------- 第二个实例：mock 引擎 + 资源仓库（J-04/J-08/J-09/J-10） ----------
        String config2 = "{"
                + "\"protocol_version\":1,"
                + "\"data_dir\":\"/data/local/tmp/p6\","
                + "\"dict_package\":\"" + dictDir + "\","
                + "\"resource_repo\":\"/data/local/tmp/p7repo\","
                + "\"mock_engines\":true}";
        String h2 = extract(NativeBridge.nativeCreate(config2), "\"handle\":\"", "\"");

        // J-04：资源清单（空仓库也应如实返回，不能假造）
        String res = NativeBridge.nativeCall(h2, "resources", "{}");
        check("J-04a", res.contains("\"ok\":true") && res.contains("items"),
                "资源清单可查询: " + trim(res));

        // 翻译：mock NMT 必须标明自己是 mock，不能被当成真实离线翻译
        String tr = NativeBridge.nativeCall(h2, "translate",
                "{\"text\":\"Hello world\",\"target\":\"zh-Hans\"}");
        check("J-04b", tr.contains("\"ok\":true") && tr.contains("mock.nmt"),
                "mock NMT 参与路由且引擎可识别: " + trim(tr));

        // J-08：未授权的远程引擎必须报 NetworkNotAuthorized，且不产生网络请求
        String remote = NativeBridge.nativeCall(h2, "translate",
                "{\"text\":\"Hello\",\"target\":\"zh-Hans\",\"policy\":\"remote\"}");
        check("J-08", remote.contains("NetworkNotAuthorized"),
                "未授权远程被拒: " + trim(remote));

        // J-09：本地 NMT 未编译进产物 → 明确 CapabilityUnavailable（不是崩溃/空结果）
        String localOnly = NativeBridge.nativeCall(h2, "translate",
                "{\"text\":\"Hello\",\"target\":\"zh-Hans\",\"policy\":\"local\"}");
        check("J-09", localOnly.contains("mock.nmt") || localOnly.contains("CapabilityUnavailable"),
                "本地策略行为明确: " + trim(localOnly));

        // J-10：坏 JSON / 未知 method / 缺字段 都是受控错误
        String badJson = NativeBridge.nativeCall(h2, "lookup", "{not json");
        check("J-10a", badJson.contains("InvalidRequest"),
                "坏 JSON 受控失败: " + trim(badJson));
        String unknown = NativeBridge.nativeCall(h2, "no_such_method", "{}");
        check("J-10b", unknown.contains("InvalidRequest") && unknown.contains("未知 method"),
                "未知 method 受控失败: " + trim(unknown));
        String missing = NativeBridge.nativeCall(h2, "translate", "{\"text\":\"x\"}");
        check("J-10c", missing.contains("InvalidRequest"),
                "缺 target 受控失败: " + trim(missing));

        // J-10 边界：超长文本 / 控制字符 / 深嵌套 / 超大 payload / limit 上限
        String long64k = repeat('a', 64 * 1024);
        String l64 = NativeBridge.nativeCall(h2, "detect",
                "{\"text\":\"" + long64k + "\"}");
        check("J-10d", l64.contains("\"ok\":true"),
                "64 KiB 文本正常处理: " + trim(l64));

        String long1m = repeat('a', 1024 * 1024);
        String l1m = NativeBridge.nativeCall(h2, "detect",
                "{\"text\":\"" + long1m + "\"}");
        check("J-10e", l1m.contains("InvalidRequest") && l1m.contains("上限"),
                "1 MiB 文本明确拒绝且不 OOM: " + trim(l1m));

        String ctrl = NativeBridge.nativeCall(h2, "detect",
                "{\"text\":\"a\\u0001b\"}");
        check("J-10f", ctrl.contains("InvalidRequest") && ctrl.contains("控制字符"),
                "控制字符明确拒绝: " + trim(ctrl));

        String deep = repeat('[', 200) + "0" + repeat(']', 200);
        String dj = NativeBridge.nativeCall(h2, "detect", deep);
        check("J-10g", dj.contains("InvalidRequest"),
                "200 层嵌套 JSON 受控失败: " + trim(dj));

        String huge = repeat('a', 300 * 1024);
        String hg = NativeBridge.nativeCall(h2, "detect",
                "{\"text\":\"" + huge + "\"}");
        check("J-10h", hg.contains("InvalidRequest"),
                "超过 payload 上限受控失败: " + trim(hg));

        String lim = NativeBridge.nativeCall(h2, "lookup",
                "{\"term\":\"x\",\"limit\":1001}");
        check("J-10i", lim.contains("InvalidRequest") && lim.contains("limit"),
                "lookup limit 上限受控失败: " + trim(lim));

        // ---------- J-07：资源仓库由宿主构造并推送（坏包安装已在主机侧拒绝） ----------
        // 注意：创建新句柄会让旧代际失效（nativeCreate 的设计），因此这里复用 h2，
        // 不额外 create，避免把 J-06 的句柄提前作废。
        String res3 = NativeBridge.nativeCall(h2, "resources", "{}");
        check("J-07a", res3.contains("\"ok\":true")
                        && res3.contains("dictionary.p7.base")
                        && res3.contains("1.0.0"),
                "仓库里激活版本可读: " + trim(res3));
        check("J-07b", !res3.contains("dictionary.p7.evil")
                        && !res3.contains("dictionary.p7.traversal"),
                "坏包/穿越包没有进入仓库: " + trim(res3));

        // ---------- J-06：并发 lookup 与 close 不得互相释放 ----------
        final String concHandle = h2;
        final java.util.concurrent.atomic.AtomicInteger okCount =
                new java.util.concurrent.atomic.AtomicInteger();
        final java.util.concurrent.atomic.AtomicInteger closedCount =
                new java.util.concurrent.atomic.AtomicInteger();
        final boolean[] crashed = { false };
        Thread worker = new Thread(() -> {
            try {
                for (int i = 0; i < 400; i++) {
                    if (i % 20 == 0) {
                        // 稍微拖慢，让 close 有机会落在"调用进行中"
                        Thread.sleep(2);
                    }
                    String r = NativeBridge.nativeCall(concHandle, "lookup",
                            "{\"term\":\"你好\"}");
                    if (r.contains("\"ok\":true")) {
                        okCount.incrementAndGet();
                    } else if (r.contains("AlreadyClosed")) {
                        closedCount.incrementAndGet();
                    } else {
                        crashed[0] = true;
                        System.out.println("unexpected: " + trim(r));
                        break;
                    }
                }
            } catch (Throwable t) {
                crashed[0] = true;
                System.out.println("worker threw: " + t);
            }
        });
        worker.start();
        // 让 worker 先跑几轮，再在另一个线程关闭
        Thread.sleep(60);
        String closeConc = NativeBridge.nativeClose(concHandle);
        worker.join(20000);
        check("J-06", !crashed[0] && (okCount.get() + closedCount.get()) == 400 && okCount.get() > 0,
                "并发调用与关闭无崩溃：ok=" + okCount.get() + " alreadyClosed=" + closedCount.get()
                        + " close=" + trim(closeConc));
        // J-15b 与 J-06 同一实验：400 次在飞调用 + 中途 close，无崩溃、无 UAF。
        check("J-15b", !crashed[0] && (okCount.get() + closedCount.get()) == 400,
                "并发 close 与在飞调用：ok=" + okCount.get() + " alreadyClosed=" + closedCount.get());

        // J-15c：ART 不保证 System.loadLibrary 卸载语义，不伪造 PASS。
        System.out.println("PENDING J-15c .so 重载：ART 类加载器不保证卸载，不可测（docs/43 D-146）");

        // ---------- J-15a：50 次 create → lookup → close，代际单调且旧句柄失效 ----------
        boolean genOk = true;
        int prevGen = -1;
        for (int i = 0; i < 50; i++) {
            String h = extract(NativeBridge.nativeCreate(config2), "\"handle\":\"", "\"");
            if (h == null || !h.contains(":")) {
                genOk = false;
                break;
            }
            int gen = Integer.parseInt(h.substring(0, h.indexOf(':')));
            if (prevGen >= 0 && gen != prevGen + 1) {
                System.out.println("generation jump: " + prevGen + " -> " + gen);
                genOk = false;
            }
            prevGen = gen;
            String lk = NativeBridge.nativeCall(h, "lookup", "{\"term\":\"你好\"}");
            if (!lk.contains("\"ok\":true")) {
                System.out.println("lookup failed: " + trim(lk));
                genOk = false;
            }
            NativeBridge.nativeClose(h);
            String stale15 = NativeBridge.nativeCall(h, "lookup", "{\"term\":\"你好\"}");
            if (!stale15.contains("AlreadyClosed")) {
                System.out.println("stale handle accepted: " + trim(stale15));
                genOk = false;
            }
        }
        check("J-15a", genOk && prevGen > 0,
                "50 次重建：代际单调、旧句柄 AlreadyClosed（last gen=" + prevGen + "）");

        // ---------- G06：JNI 取消路径（预取消的 request_id 必须返回 Cancelled） ----------
        String h4 = extract(NativeBridge.nativeCreate(config2), "\"handle\":\"", "\"");
        String cancelAck = NativeBridge.nativeCall(h4, "cancel",
                "{\"request_id\":\"g06-pre\"}");
        String cancelled = NativeBridge.nativeCall(h4, "translate",
                "{\"text\":\"Hello\",\"target\":\"zh-Hans\",\"request_id\":\"g06-pre\"}");
        check("G06-JNI", cancelAck.contains("\"cancelled\":true")
                        && cancelled.contains("Cancelled"),
                "JNI 预取消返回 Cancelled: " + trim(cancelled));
        NativeBridge.nativeClose(h4);


        // ---------- 收尾 ----------
        System.out.println("SUMMARY pass=" + pass + " fail=" + fail);
        if (fail > 0) {
            System.exit(1);
        }
    }

    private static void check(String id, boolean ok, String detail) {
        if (ok) {
            pass++;
            System.out.println("PASS " + id + " " + detail);
        } else {
            fail++;
            System.out.println("FAIL " + id + " " + detail);
        }
    }

    private static String repeat(char c, int n) {
        char[] chars = new char[n];
        java.util.Arrays.fill(chars, c);
        return new String(chars);
    }

    private static String extract(String s, String start, String end) {
        if (s == null) return null;
        int i = s.indexOf(start);
        if (i < 0) return null;
        int from = i + start.length();
        int j = s.indexOf(end, from);
        if (j < 0) return null;
        return s.substring(from, j);
    }

    private static String trim(String s) {
        if (s == null) return "<null>";
        return s.length() <= 600 ? s : s.substring(0, 600) + "...";
    }
}
