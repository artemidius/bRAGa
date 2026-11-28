// SPDX-License-Identifier: Apache-2.0

using System.Text;
using Xunit;

namespace BragaNet.Tests;

/// <summary>Conformance tests against the real <c>braga_ffi</c> cdylib, in parity with the other wrappers.</summary>
public sealed class BragaTests
{
    [Fact]
    public void AbiVersionIsOne()
    {
        Assert.Equal(1u, Braga.AbiVersion());
        Assert.Equal(1u, NativeLib.AbiVersion);
        Assert.Equal(1u, NativeMethods.braga_abi_version());
    }

    [Fact]
    public void EnumsMatchHeader()
    {
        Dictionary<BragaResult, int> results = new()
        {
            [BragaResult.Ok] = 0,
            [BragaResult.InvalidArg] = 1,
            [BragaResult.Config] = 2,
            [BragaResult.Key] = 3,
            [BragaResult.Store] = 4,
            [BragaResult.Embed] = 5,
            [BragaResult.NotFound] = 6,
            [BragaResult.Io] = 7,
            [BragaResult.Panic] = 8,
            [BragaResult.Oom] = 9,
        };
        foreach ((BragaResult result, int code) in results)
        {
            Assert.Equal(code, (int)result);
        }

        Dictionary<BragaStatus, int> statuses = new()
        {
            [BragaStatus.UpToDate] = 0,
            [BragaStatus.Checking] = 1,
            [BragaStatus.Updating] = 2,
            [BragaStatus.Error] = 3,
        };
        foreach ((BragaStatus status, int code) in statuses)
        {
            Assert.Equal(code, (int)status);
        }

        Assert.Equal(BragaResult.Panic, BragaResultExtensions.FromCode(99));
        Assert.Equal(BragaResult.Panic, BragaResultExtensions.FromCode(-1));
        Assert.Equal(BragaStatus.Error, BragaStatusExtensions.FromCode(99));
        Assert.Equal(BragaStatus.Error, BragaStatusExtensions.FromCode(-1));
    }

    [Fact]
    public void RoundTrip()
    {
        using Harness harness = new();
        using Braga braga = Braga.Open(harness.Config, harness.Key);

        Assert.Equal(BragaStatus.UpToDate, braga.Status());
        RequireError(() => braga.Query(Harness.Query, 1), BragaResult.NotFound);

        braga.Update();
        Assert.Equal(BragaStatus.UpToDate, braga.Status());

        Hit hit = Assert.Single(braga.Query(Harness.Query, 1));
        Assert.Matches("^[0-9a-f]{64}$", hit.ChunkId);
        Assert.EndsWith("notes.md", hit.Path);
        Assert.Contains("quick brown fox", hit.Content);
        Assert.True(hit.LineStart >= 1, $"line_start = {hit.LineStart}");
        Assert.True(hit.LineEnd >= hit.LineStart, $"line range = {hit.LineStart}..{hit.LineEnd}");
        Assert.True(hit.Score > 0, $"score = {hit.Score}");

        DoctorReport report = braga.Doctor();
        Assert.True(report.Ok);
        Assert.Equal(6, report.Checks.Count);
        foreach (DoctorCheck check in report.Checks)
        {
            Assert.True(check.Passed, $"{check.Name}: {check.Detail}");
        }

        braga.Dispose();
        braga.Dispose();
        RequireError(() => braga.Query(Harness.Query, 1), BragaResult.InvalidArg);
        Assert.Equal(BragaStatus.Error, braga.Status());
    }

    [Fact]
    public void WrongKey()
    {
        using Harness harness = new();
        using (Braga.Open(harness.Config, harness.Key))
        {
        }

        string wrong = harness.Write("wrong-key", Enumerable.Repeat((byte)0x11, 32).ToArray());
        BragaException error = RequireError(() => Braga.Open(harness.Config, wrong), BragaResult.Store);
        Assert.Contains("wrong database key", error.Message);
    }

    [Fact]
    public void MalformedKey()
    {
        using Harness harness = new();
        string bad = harness.Write("bad-key", Encoding.UTF8.GetBytes("not-a-key"));

        BragaException error = RequireError(() => Braga.Open(harness.Config, bad), BragaResult.Key);
        Assert.Contains("expected exactly 32 bytes", error.Message);
        Assert.DoesNotContain("not-a-key", error.Message);
    }

    [Fact]
    public void MissingConfig()
    {
        using Harness harness = new();
        BragaException error = RequireError(
            () => Braga.Open(Path.Combine(harness.Root, "missing.toml"), harness.Key),
            BragaResult.Config);
        Assert.Contains("configuration", error.Message);
    }

    [Fact]
    public void BrokenToml()
    {
        using Harness harness = new();
        string broken = harness.Write(
            "broken.toml",
            Encoding.UTF8.GetBytes("[scan]\ndir = \"SUPER-SECRET\" nope\n"));

        BragaException error = RequireError(() => Braga.Open(broken, harness.Key), BragaResult.Config);
        Assert.Contains("invalid configuration", error.Message);
        Assert.DoesNotContain("SUPER-SECRET", error.Message);
    }

    [Fact]
    public void InitRefusesToClobber()
    {
        using Harness harness = new();
        string config = Path.Combine(harness.Root, "init.toml");
        string key = Path.Combine(harness.Root, "nested", "init-key");

        Braga.Init(config, key);
        Assert.True(File.Exists(config));
        Assert.Equal(32, new FileInfo(key).Length);

        BragaException error = RequireError(() => Braga.Init(config, key), BragaResult.Io);
        Assert.Contains("already exists", error.Message);
    }

    [Fact]
    public unsafe void NoCrash()
    {
        // These calls bypass the typed facade, so pin the library before touching the raw ABI.
        NativeLib.EnsureLoaded();

        Assert.Equal(BragaResult.InvalidArg, BragaResultExtensions.FromCode(NativeMethods.braga_close(0)));
        Assert.Equal(BragaStatus.Error, BragaStatusExtensions.FromCode(NativeMethods.braga_status(0)));
        Assert.Equal(BragaResult.InvalidArg, BragaResultExtensions.FromCode(NativeMethods.braga_update(0, null, 0)));

        nint json = 0;
        nuint length = 0;
        Assert.Equal(
            BragaResult.InvalidArg,
            BragaResultExtensions.FromCode(NativeMethods.braga_doctor(0, (byte**)&json, &length)));

        byte[] query = Encoding.UTF8.GetBytes(Harness.Query);
        fixed (byte* text = query)
        {
            Assert.Equal(
                BragaResult.InvalidArg,
                BragaResultExtensions.FromCode(
                    NativeMethods.braga_query(0, text, (nuint)query.Length, 1, null, 0, (byte**)&json, &length)));
        }

        using Harness harness = new();
        using Braga braga = Braga.Open(harness.Config, harness.Key);
        nint handle = braga.NativeHandle;

        byte[]?[] texts = [null, [], [0xFF, 0xFE]];
        foreach (byte[]? payload in texts)
        {
            nint outJson = 0;
            nuint outLength = 0;
            int code;
            fixed (byte* text = payload)
            {
                code = NativeMethods.braga_query(
                    handle,
                    text,
                    payload is null ? 0 : (nuint)payload.Length,
                    1,
                    null,
                    0,
                    (byte**)&outJson,
                    &outLength);
            }

            Assert.Equal(BragaResult.InvalidArg, BragaResultExtensions.FromCode(code));
            Assert.Equal(0L, (long)outJson);
            Assert.Equal(0L, (long)outLength);
        }

        RequireError(() => braga.Query("\uD800"), BragaResult.InvalidArg);
        RequireError(() => braga.Query(Harness.Query, -1), BragaResult.InvalidArg);

        braga.Update();
        nint maxJson = 0;
        nuint maxLength = 0;
        int maxCode;
        fixed (byte* text = query)
        {
            maxCode = NativeMethods.braga_query(
                handle, text, (nuint)query.Length, uint.MaxValue, null, 0, (byte**)&maxJson, &maxLength);
        }

        BragaResult maxResult = BragaResultExtensions.FromCode(maxCode);
        Assert.NotEqual(BragaResult.Panic, maxResult);
        Assert.True(maxResult == BragaResult.Ok ? maxJson != 0 : maxJson == 0);
        if (maxResult == BragaResult.Ok)
        {
            NativeMethods.braga_string_free((byte*)maxJson);
        }
    }

    private static BragaException RequireError(Action action, BragaResult code)
    {
        BragaException error = Assert.Throws<BragaException>(action);
        Assert.Equal(code, error.Code);
        return error;
    }
}
