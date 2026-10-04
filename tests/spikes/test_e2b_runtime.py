import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest

spec=importlib.util.spec_from_file_location('runtime_worker',Path(__file__).parents[2]/'scripts/e2b-runtime.py')
worker=importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)
LABELS={key:'test' for key in ('semon_version','semon_deployment','semon_owner','semon_workspace','semon_session','semon_operation','semon_epoch')}

class Provider:
    calls=[]
    labels=LABELS
    state='running'
    @classmethod
    def create(cls,**args):
        cls.calls.append(args)
        return SimpleNamespace(sandbox_id='owned-runtime')
    @classmethod
    def get_info(cls,*args,**kwargs):
        return SimpleNamespace(metadata=cls.labels,state=cls.state)
    def __init__(self,*args,**kwargs):
        raise AssertionError('guest access must not occur')

class RuntimeTests(unittest.TestCase):
    def setUp(self):
        Provider.calls=[]
        Provider.labels=LABELS
        Provider.state='running'
    def request(self,method='create',runtime=None,payload=None):
        return dict(method=method,api_key='synthetic-secret',runtime=runtime,labels=LABELS,payload=({'template':'base','timeout':3600,'keep_memory':True} if payload is None and method=='create' else payload or {}))
    def test_only_explicit_create_can_provision(self):
        result=worker.execute(self.request(),Provider)
        self.assertEqual(result['runtime'],'owned-runtime')
        self.assertEqual(len(Provider.calls),1)
        self.assertEqual(Provider.calls[0]['lifecycle']['on_timeout']['action'],'pause')
        self.assertNotIn('envs',Provider.calls[0])
        self.assertNotIn('synthetic-secret',str(result))
    def test_create_is_never_retried_after_failure(self):
        class Failing(Provider):
            @classmethod
            def create(cls,**args):
                cls.calls.append(args)
                raise TimeoutError('uncertain create')
        with self.assertRaises(TimeoutError):worker.execute(self.request(),Failing)
        self.assertEqual(len(Provider.calls),1)
    def test_missing_or_invalid_profile_never_provisions(self):
        for profile in ({}, {'template':'base','timeout':True,'keep_memory':True}, {'template':'base','timeout':3600,'keep_memory':'yes'}):
            self.assertEqual(worker.execute(self.request(payload=profile),Provider),worker.UNKNOWN)
        self.assertEqual(Provider.calls,[])
    def test_bound_create_is_refused(self):
        self.assertEqual(worker.execute(self.request(runtime='owned-runtime'),Provider),worker.UNKNOWN)
        self.assertEqual(Provider.calls,[])
    def test_wrong_owner_and_paused_compute_never_bootstrap_or_dispatch(self):
        for method in ('bootstrap','dispatch','inspect'):
            Provider.labels={**LABELS,'semon_owner':'another-owner'}
            self.assertEqual(worker.execute(self.request(method,'owned-runtime'),Provider),worker.UNKNOWN)
            Provider.labels=LABELS
            Provider.state='paused'
            self.assertEqual(worker.execute(self.request(method,'owned-runtime'),Provider),worker.UNKNOWN)
            Provider.state='running'
        self.assertEqual(Provider.calls,[])

if __name__=='__main__':unittest.main()
