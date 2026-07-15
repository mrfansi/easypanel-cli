<?php
// tests/Unit/EasypanelClientTest.php
use App\Support\EasypanelClient;
use App\Support\EasypanelException;
use Illuminate\Support\Facades\Http;

it('posts to the rpc path with bearer auth and json envelope, unwrapping .json', function () {
    Http::fake([
        '*' => Http::response(['json' => [['name' => 'proj-a']], 'meta' => []]),
    ]);

    $result = (new EasypanelClient('https://panel.test/', 'tok123'))->call('projects', 'listProjects');

    expect($result)->toBe([['name' => 'proj-a']]);

    Http::assertSent(fn ($req) => $req->url() === 'https://panel.test/api/rpc/projects/listProjects'
        && $req->method() === 'POST'
        && $req->hasHeader('Authorization', 'Bearer tok123')
        && $req->data() === ['json' => null]);
});

it('sends the given input wrapped in json', function () {
    Http::fake(['*' => Http::response(['json' => ['ok' => true]])]);

    (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'createProject', ['name' => 'proj-a']);

    Http::assertSent(fn ($req) => $req->data() === ['json' => ['name' => 'proj-a']]);
});

it('throws EasypanelException with a friendly message on 401', function () {
    Http::fake(['*' => Http::response(['message' => 'Unauthorized'], 401)]);

    expect(fn () => (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'listProjects'))
        ->toThrow(EasypanelException::class, 'Token tidak valid');
});

it('throws EasypanelException surfacing the api message on other errors', function () {
    Http::fake(['*' => Http::response(['message' => 'Boom'], 500)]);

    expect(fn () => (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'listProjects'))
        ->toThrow(EasypanelException::class, 'Boom');
});
